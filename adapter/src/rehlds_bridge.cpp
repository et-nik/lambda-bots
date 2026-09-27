#include "rehlds_bridge.h"

#if defined(LB_WITH_REHLDS)

#include <cstring>

#include "capture.h"
#include "rehlds_api.h"

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <dlfcn.h>
#endif

namespace lb {

namespace {

struct Bridge {
    rehlds::IRehldsApi *api = nullptr;
    const rehlds::RehldsFuncs_t *funcs = nullptr;
    rehlds::IRehldsServerStatic *svs = nullptr;
    rehlds::IRehldsServerData *sv = nullptr;
    rehlds::IMessageManager *msgmgr = nullptr;
    rehlds::StartSoundRegistry *start_sound = nullptr;
    int minor = 0;
    int build = 0;
    uint8_t hooked[32] = {};
};

Bridge &br() {
    static Bridge b;
    return b;
}

using CreateInterfaceFn = void *(*)(const char *name, int *code);

CreateInterfaceFn engine_factory() {
#if defined(_WIN32)
    for (const char *name : {"swds.dll", "sw.dll", "hw.dll"}) {
        if (HMODULE m = GetModuleHandleA(name)) {
            return reinterpret_cast<CreateInterfaceFn>(GetProcAddress(m, "CreateInterface"));
        }
    }
    return nullptr;
#else
    void *m = dlopen("engine_i486.so", RTLD_NOW | RTLD_NOLOAD);
    if (!m) return nullptr;
    auto factory = reinterpret_cast<CreateInterfaceFn>(dlsym(m, "CreateInterface"));
    dlclose(m);
    return factory;
#endif
}

void on_start_sound(rehlds::StartSoundChain *chain, int recipients, edict_t *ent, int channel, const char *sample,
                    int volume, float attn, int flags, int pitch) {
    if (ent && sample) {
        float origin[3];
        for (int i = 0; i < 3; i++) origin[i] = ent->v.origin[i] + (ent->v.mins[i] + ent->v.maxs[i]) * 0.5f;
        record_sound(LB_SOUND_SRC_REHLDS, ent, channel, sample, origin, static_cast<float>(volume) / 255.0f, attn,
                     flags, pitch);
    }
    chain->callNext(recipients, ent, channel, sample, volume, attn, flags, pitch);
}

bool origin_matters(rehlds::IMessage::Dest d) {
    using D = rehlds::IMessage::Dest;
    return d == D::PVS || d == D::PAS || d == D::PVS_R || d == D::PAS_R;
}

void on_message(rehlds::IVoidHookChain<rehlds::IMessage *> *chain, rehlds::IMessage *msg) {
    using P = rehlds::IMessage::ParamType;
    const rehlds::IMessage::Dest dest = msg->getDest();
    capture_message_begin(MsgSource::MessageManager, static_cast<int>(dest), msg->getId(),
                          origin_matters(dest) ? msg->getOrigin() : nullptr, msg->getEdict());
    const int count = msg->getParamCount();
    for (int i = 0; i < count; i++) {
        switch (msg->getParamType(i)) {
            case P::Byte: capture_arg_int(LB_MSG_ARG_BYTE, msg->getParamInt(i)); break;
            case P::Char: capture_arg_int(LB_MSG_ARG_CHAR, msg->getParamInt(i)); break;
            case P::Short: capture_arg_int(LB_MSG_ARG_SHORT, msg->getParamInt(i)); break;
            case P::Long: capture_arg_int(LB_MSG_ARG_LONG, msg->getParamInt(i)); break;
            case P::Entity: capture_arg_int(LB_MSG_ARG_ENTITY, msg->getParamInt(i)); break;
            case P::Angle: capture_arg_float(LB_MSG_ARG_ANGLE, msg->getParamFloat(i)); break;
            case P::Coord: capture_arg_float(LB_MSG_ARG_COORD, msg->getParamFloat(i)); break;
            case P::String: capture_arg_string(msg->getParamString(i)); break;
        }
    }
    capture_message_end();
    chain->callNext(msg);
}

// GetBuildNumber is the engine's date-based build (e.g. 4419), not a commit count, so only the API minor counts.
bool has_message_manager(const Bridge &b) {
    return b.minor >= rehlds::kMessageManagerMinor;
}

void unhook_messages() {
    Bridge &b = br();
    if (!b.msgmgr) return;
    for (int id = 0; id < 256; id++) {
        if (b.hooked[id / 8] & (1u << (id % 8))) b.msgmgr->unregisterHook(id, on_message);
    }
    std::memset(b.hooked, 0, sizeof(b.hooked));
    capture_set_msgmgr_mask(b.hooked);
}

}  // namespace

bool rehlds_init() {
    Bridge &b = br();
    if (b.api) return true;
    CreateInterfaceFn factory = engine_factory();
    if (!factory) return false;
    int code = 0;
    auto *api = static_cast<rehlds::IRehldsApi *>(factory(rehlds::kInterfaceVersion, &code));
    if (!api) return false;
    const int major = api->GetMajorVersion();
    const int minor = api->GetMinorVersion();
    if (major != rehlds::kMajor || minor < rehlds::kMinMinor) {
        log_console("ReHLDS API %d.%d is not supported (need %d.%d+); ReHLDS channels are off\n", major, minor,
                    rehlds::kMajor, rehlds::kMinMinor);
        return false;
    }
    b.api = api;
    b.minor = minor;
    b.funcs = api->GetFuncs();
    b.svs = api->GetServerStatic();
    b.sv = api->GetServerData();
    b.build = b.funcs->GetBuildNumber();
    b.msgmgr = has_message_manager(b) ? api->GetMessageManager() : nullptr;
    b.start_sound = api->GetHookchains()->SV_StartSound();
    b.start_sound->registerHook(on_start_sound, rehlds::HC_PRIORITY_LOW);
    return true;
}

void rehlds_shutdown() {
    Bridge &b = br();
    if (!b.api) return;
    unhook_messages();
    if (b.start_sound) b.start_sound->unregisterHook(on_start_sound);
    b = Bridge{};
}

void rehlds_fill_compat(LbCompatFacts *out) {
    const Bridge &b = br();
    if (!b.api) return;
    out->engine_kind = LB_ENGINE_REHLDS;
    out->rehlds_major = static_cast<uint8_t>(rehlds::kMajor);
    out->rehlds_minor = static_cast<uint8_t>(b.minor);
    out->rehlds_build = b.build;
    out->channels |= LB_CH_SV_STARTSOUND | LB_CH_DROPCLIENT | LB_CH_HOSTTIME;
    if (b.msgmgr) out->channels |= LB_CH_MSGMGR;
}

bool rehlds_sound_channel() {
    return br().start_sound != nullptr;
}

bool rehlds_hook_messages(const uint8_t mask[32]) {
    Bridge &b = br();
    if (!b.msgmgr) return false;
    for (int id = 0; id < 256; id++) {
        const uint8_t bit = static_cast<uint8_t>(1u << (id % 8));
        const bool want = (mask[id / 8] & bit) != 0;
        const bool have = (b.hooked[id / 8] & bit) != 0;
        if (want && !have) {
            b.msgmgr->registerHook(id, on_message, rehlds::HC_PRIORITY_LOW);
            b.hooked[id / 8] |= bit;
        } else if (!want && have) {
            b.msgmgr->unregisterHook(id, on_message);
            b.hooked[id / 8] &= static_cast<uint8_t>(~bit);
        }
    }
    capture_set_msgmgr_mask(b.hooked);
    return true;
}

bool rehlds_drop_client(int slot, const char *reason) {
    Bridge &b = br();
    if (!b.svs || slot < 1 || slot > b.svs->GetMaxClients()) return false;
    rehlds::IGameClient *cl = b.svs->GetClient(slot - 1);
    if (!cl) return false;
    b.funcs->DropClient(cl, false, "%s", reason ? reason : "");
    return true;
}

bool rehlds_time(double *sim_time, double *frame_time) {
    const Bridge &b = br();
    if (!b.api) return false;
    *sim_time = b.sv->GetTime();
    *frame_time = b.funcs->GetHostFrameTime();
    return true;
}

}  // namespace lb

#else

namespace lb {

bool rehlds_init() {
    return false;
}

void rehlds_shutdown() {}

void rehlds_fill_compat(LbCompatFacts *) {}

bool rehlds_sound_channel() {
    return false;
}

bool rehlds_hook_messages(const uint8_t *) {
    return false;
}

bool rehlds_drop_client(int, const char *) {
    return false;
}

bool rehlds_time(double *, double *) {
    return false;
}

}  // namespace lb

#endif
