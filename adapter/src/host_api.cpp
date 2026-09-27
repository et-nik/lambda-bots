// LbHostApi: the functions the Rust core may call (main thread, inside lb_core_* calls only).
#include "host_api.h"

#include <string>
#include <vector>

#include "capture.h"
#include "fake_client.h"
#include "registry.h"
#include "rehlds_bridge.h"

namespace lb {

namespace {

constexpr int kMaxCvars = 256;

struct CvarSlots {
    cvar_t storage[kMaxCvars];
    char names[kMaxCvars][64];
    char defaults[kMaxCvars][256];
    int used = 0;
};

CvarSlots g_cvar_slots;
std::vector<cvar_t *> g_cvar_handles;

std::string to_string(LbStr s) {
    return std::string(reinterpret_cast<const char *>(s.ptr ? s.ptr : reinterpret_cast<const uint8_t *>("")), s.len);
}

uint16_t handle_for(cvar_t *cv) {
    for (size_t i = 0; i < g_cvar_handles.size(); i++) {
        if (g_cvar_handles[i] == cv) return static_cast<uint16_t>(i + 1);
    }
    if (g_cvar_handles.size() >= 0xFFFE) return 0;
    g_cvar_handles.push_back(cv);
    return static_cast<uint16_t>(g_cvar_handles.size());
}

cvar_t *cvar_of(uint16_t h) {
    return (h >= 1 && h <= g_cvar_handles.size()) ? g_cvar_handles[h - 1] : nullptr;
}

uint32_t copy_out(const char *src, uint8_t *buf, uint32_t cap) {
    if (!src || !buf || cap == 0) return 0;
    const size_t n = std::strlen(src);
    const uint32_t len = static_cast<uint32_t>(n < cap ? n : cap);
    std::memcpy(buf, src, len);
    return len;
}

void h_server_print(void *, LbStr text) {
    std::string s = to_string(text);
    g_engfuncs.pfnServerPrint(s.c_str());
}

void h_client_print(void *, uint8_t slot, uint8_t where, LbStr text) {
    if (slot < 1 || slot > state().max_clients) return;
    edict_t *ed = edict_of(slot);
    if (!ed || ed->free || (ed->v.flags & FL_FAKECLIENT)) return;
    const PRINT_TYPE type = where == LB_PRINT_CENTER ? print_center : (where == LB_PRINT_CHAT ? print_chat : print_console);
    std::string s = to_string(text);
    g_engfuncs.pfnClientPrintf(ed, type, s.c_str());
}

void h_server_command(void *, LbStr text) {
    std::string s = to_string(text);
    if (s.empty() || s.back() != '\n') s += '\n';
    g_engfuncs.pfnServerCommand(s.c_str());
}

int32_t h_cvar_register(void *, const LbCvarSpec *specs, uint32_t count, uint16_t *out) {
    for (uint32_t i = 0; i < count; i++) {
        out[i] = 0;
        const std::string name = to_string(specs[i].name);
        if (cvar_t *existing = g_engfuncs.pfnCVarGetPointer(name.c_str())) {
            out[i] = handle_for(existing);
            continue;
        }
        if (g_cvar_slots.used >= kMaxCvars || name.size() >= 64) continue;
        const int k = g_cvar_slots.used++;
        std::snprintf(g_cvar_slots.names[k], sizeof(g_cvar_slots.names[k]), "%s", name.c_str());
        std::snprintf(g_cvar_slots.defaults[k], sizeof(g_cvar_slots.defaults[k]), "%s", to_string(specs[i].default_value).c_str());
        cvar_t &cv = g_cvar_slots.storage[k];
        std::memset(&cv, 0, sizeof(cv));
        cv.name = g_cvar_slots.names[k];
        cv.string = g_cvar_slots.defaults[k];
        cv.flags = FCVAR_EXTDLL;
        if (specs[i].flags & LB_CVAR_SERVER) cv.flags |= FCVAR_SERVER;
        if (specs[i].flags & LB_CVAR_PROTECTED) cv.flags |= FCVAR_PROTECTED;
        if (specs[i].flags & LB_CVAR_READONLY) cv.flags |= FCVAR_SPONLY;
        g_engfuncs.pfnCVarRegister(&cv);
        cvar_t *registered = g_engfuncs.pfnCVarGetPointer(name.c_str());
        if (registered) out[i] = handle_for(registered);
    }
    return LB_OK;
}

uint16_t h_cvar_find(void *, LbStr name) {
    const std::string n = to_string(name);
    cvar_t *cv = g_engfuncs.pfnCVarGetPointer(n.c_str());
    return cv ? handle_for(cv) : 0;
}

float h_cvar_get_float(void *, uint16_t h) {
    cvar_t *cv = cvar_of(h);
    return cv ? cv->value : 0.0f;
}

uint32_t h_cvar_get_string(void *, uint16_t h, uint8_t *buf, uint32_t cap) {
    cvar_t *cv = cvar_of(h);
    return cv ? copy_out(cv->string, buf, cap) : 0;
}

void h_cvar_set(void *, uint16_t h, LbStr value) {
    cvar_t *cv = cvar_of(h);
    if (!cv) return;
    const std::string v = to_string(value);
    g_engfuncs.pfnCvar_DirectSet(cv, v.c_str());
}

void trace_one(const LbTraceRequest *req, LbTraceResult *out) {
    TraceResult tr{};
    const float v1[3] = {req->start.x, req->start.y, req->start.z};
    const float v2[3] = {req->end.x, req->end.y, req->end.z};
    int no_monsters = (req->flags & LB_TRACE_IGNORE_MONSTERS) ? 1 : 0;
    if (req->flags & LB_TRACE_MISSILE) no_monsters = 2;
    if (req->flags & LB_TRACE_IGNORE_GLASS) no_monsters |= 0x100;
    edict_t *skip = nullptr;
    if (req->ignore.index) {
        edict_t *ed = edict_of(req->ignore.index);
        // Player edicts are never reallocated, so a slot needs no serial check.
        if (ed && (is_player_edict(ed) || static_cast<uint32_t>(ed->serialnumber) == req->ignore.serial)) skip = ed;
    }
    switch (req->kind) {
        case LB_TRACE_HULL:
            g_engfuncs.pfnTraceHull(v1, v2, no_monsters, req->hull, skip, &tr);
            break;
        case LB_TRACE_MODEL: {
            edict_t *model = edict_of(req->model.index);
            if (!model || static_cast<uint32_t>(model->serialnumber) != req->model.serial) {
                tr.flFraction = 1.0f;
                std::memcpy(tr.vecEndPos, v2, sizeof(v2));
                break;
            }
            g_engfuncs.pfnTraceModel(v1, v2, req->hull, model, &tr);
            break;
        }
        default:
            g_engfuncs.pfnTraceLine(v1, v2, no_monsters, skip, &tr);
            break;
    }
    std::memset(out, 0, sizeof(*out));
    out->fraction = tr.flFraction;
    out->end_pos = LbVec3{tr.vecEndPos[0], tr.vecEndPos[1], tr.vecEndPos[2]};
    out->plane_normal = LbVec3{tr.vecPlaneNormal[0], tr.vecPlaneNormal[1], tr.vecPlaneNormal[2]};
    out->plane_dist = tr.flPlaneDist;
    out->hit = ent_ref(tr.pHit);
    out->hitgroup = tr.iHitgroup;
    out->all_solid = tr.fAllSolid ? 1 : 0;
    out->start_solid = tr.fStartSolid ? 1 : 0;
    out->in_open = tr.fInOpen ? 1 : 0;
    out->in_water = tr.fInWater ? 1 : 0;
    if (tr.pHit) {
        out->hit_rendermode = static_cast<uint8_t>(tr.pHit->v.rendermode);
        out->hit_renderfx = static_cast<uint8_t>(tr.pHit->v.renderfx);
        out->hit_renderamt = tr.pHit->v.renderamt;
        out->hit_solid = static_cast<uint8_t>(tr.pHit->v.solid);
        out->hit_classname_id = intern(lb_string(tr.pHit->v.classname));
    }
}

void h_trace(void *, const LbTraceRequest *req, LbTraceResult *out) {
    trace_one(req, out);
}

uint32_t h_trace_batch(void *, const LbTraceRequest *reqs, LbTraceResult *out, uint32_t count) {
    for (uint32_t i = 0; i < count; i++) trace_one(&reqs[i], &out[i]);
    return count;
}

int32_t h_point_contents(void *, LbVec3 p) {
    const float v[3] = {p.x, p.y, p.z};
    return g_engfuncs.pfnPointContents(v);
}

int32_t h_registry_set_rules(void *, const LbTrackRule *rules, uint32_t count) {
    return registry_set_rules(rules, count);
}

uint32_t h_snapshot_entities(void *, uint32_t kind_mask, LbEntitySnapshot *out, uint32_t cap) {
    return registry_snapshot(kind_mask, out, cap);
}

int32_t h_get_entity(void *, LbEntRef ref, LbEntitySnapshot *out) {
    return registry_get(ref, out);
}

int32_t h_set_capture_mask(void *, const uint8_t *mask) {
    capture_set_mask(mask);
    rehlds_hook_messages(mask);
    return LB_OK;
}

int32_t h_resolve_user_msg(void *, LbStr name, int32_t *size) {
    const std::string n = to_string(name);
    int sz = 0;
    int id = 0;
    if (state().util && state().util->get_user_msg_id) id = state().util->get_user_msg_id(&state().plugin_info, n.c_str(), &sz);
    if (size) *size = sz;
    return id;
}

int32_t h_create_bot(void *, const LbCreateBotRequest *req, LbCreateBotResult *out) {
    return create_bot(req, out);
}

int32_t h_kick_bot(void *, uint8_t slot, uint32_t gen, LbStr reason) {
    return kick_bot(slot, gen, reason);
}

int32_t h_bot_client_commands(void *, const LbClientCommand *cmds, uint32_t count) {
    return bot_client_commands(cmds, count);
}

int32_t h_run_player_moves(void *, const LbBotCommand *cmds, uint32_t count, LbMoveFeedback *fb) {
    return run_player_moves(cmds, count, fb);
}

uint32_t h_get_physics_key(void *, uint8_t slot, LbStr key, uint8_t *buf, uint32_t cap) {
    if (slot < 1 || slot > state().max_clients) return 0;
    const std::string k = to_string(key);
    return copy_out(g_engfuncs.pfnGetPhysicsKeyValue(edict_of(slot), k.c_str()), buf, cap);
}

uint32_t h_get_client_info_key(void *, uint8_t slot, LbStr key, uint8_t *buf, uint32_t cap) {
    if (slot < 1 || slot > state().max_clients) return 0;
    const std::string k = to_string(key);
    char *info = g_engfuncs.pfnGetInfoKeyBuffer(edict_of(slot));
    return info ? copy_out(g_engfuncs.pfnInfoKeyValue(info, k.c_str()), buf, cap) : 0;
}

int32_t h_get_weapon_data(void *, uint8_t, void *, uint32_t) {
    return LB_ERR_UNSUPPORTED;
}

int32_t h_set_bot_disguise(void *, const LbDisguise *) {
    return LB_ERR_UNSUPPORTED;
}

int32_t h_get_player_stats(void *, uint8_t slot, int32_t *ping, int32_t *loss) {
    if (slot < 1 || slot > state().max_clients) return LB_ERR_INVALID;
    int p = 0;
    int l = 0;
    g_engfuncs.pfnGetPlayerStats(edict_of(slot), &p, &l);
    *ping = p;
    *loss = l;
    return LB_OK;
}

int32_t h_load_file(void *, LbStr path, LbOwnedBuffer *out) {
    const std::string p = to_string(path);
    int len = 0;
    byte *data = g_engfuncs.pfnLoadFileForMe(const_cast<char *>(p.c_str()), &len);
    if (!data) return LB_ERR_NOT_FOUND;
    out->ptr = data;
    out->len = static_cast<uint32_t>(len);
    out->token = data;
    return LB_OK;
}

void h_free_file(void *, LbOwnedBuffer *buf) {
    if (buf && buf->token) g_engfuncs.pfnFreeFile(buf->token);
    if (buf) buf->token = nullptr;
}

int32_t h_send_debug(void *, uint8_t slot, const LbDebugPrim *prims, uint32_t count) {
    if (slot < 1 || slot > state().max_clients) return LB_ERR_INVALID;
    edict_t *ed = edict_of(slot);
    if (!ed || ed->free || (ed->v.flags & FL_FAKECLIENT)) return LB_ERR_INVALID;
    capture_set_own_send(true);
    uint32_t sent = 0;
    for (uint32_t i = 0; i < count && sent < 30; i++) {
        const LbDebugPrim &p = prims[i];
        if (p.kind == LB_DEBUG_LINE && state().beam_sprite) {
            g_engfuncs.pfnMessageBegin(MSG_ONE_UNRELIABLE, SVC_TEMPENTITY, nullptr, ed);
            g_engfuncs.pfnWriteByte(TE_BEAMPOINTS);
            g_engfuncs.pfnWriteCoord(p.a.x);
            g_engfuncs.pfnWriteCoord(p.a.y);
            g_engfuncs.pfnWriteCoord(p.a.z);
            g_engfuncs.pfnWriteCoord(p.b2.x);
            g_engfuncs.pfnWriteCoord(p.b2.y);
            g_engfuncs.pfnWriteCoord(p.b2.z);
            g_engfuncs.pfnWriteShort(state().beam_sprite);
            g_engfuncs.pfnWriteByte(0);
            g_engfuncs.pfnWriteByte(10);
            g_engfuncs.pfnWriteByte(p.life_ds ? p.life_ds : 10);
            g_engfuncs.pfnWriteByte(p.width ? p.width : 10);
            g_engfuncs.pfnWriteByte(0);
            g_engfuncs.pfnWriteByte(p.r);
            g_engfuncs.pfnWriteByte(p.g);
            g_engfuncs.pfnWriteByte(p.b);
            g_engfuncs.pfnWriteByte(p.brightness);
            g_engfuncs.pfnWriteByte(0);
            g_engfuncs.pfnMessageEnd();
            sent++;
        } else if (p.kind == LB_DEBUG_TEXT && p.text.len) {
            std::string text = to_string(p.text);
            if (text.size() > 500) text.resize(500);
            g_engfuncs.pfnMessageBegin(MSG_ONE_UNRELIABLE, SVC_TEMPENTITY, nullptr, ed);
            g_engfuncs.pfnWriteByte(TE_TEXTMESSAGE);
            g_engfuncs.pfnWriteByte(p.channel & 0x3);
            g_engfuncs.pfnWriteShort(static_cast<int>(0.02f * 8192));
            g_engfuncs.pfnWriteShort(static_cast<int>(0.2f * 8192));
            g_engfuncs.pfnWriteByte(0);
            g_engfuncs.pfnWriteByte(p.r);
            g_engfuncs.pfnWriteByte(p.g);
            g_engfuncs.pfnWriteByte(p.b);
            g_engfuncs.pfnWriteByte(255);
            g_engfuncs.pfnWriteByte(p.r);
            g_engfuncs.pfnWriteByte(p.g);
            g_engfuncs.pfnWriteByte(p.b);
            g_engfuncs.pfnWriteByte(255);
            g_engfuncs.pfnWriteShort(0);
            g_engfuncs.pfnWriteShort(0);
            g_engfuncs.pfnWriteShort(static_cast<int>((p.life_ds ? p.life_ds : 10) / 10.0f * 256));
            g_engfuncs.pfnWriteString(text.c_str());
            g_engfuncs.pfnMessageEnd();
            sent++;
        }
    }
    capture_set_own_send(false);
    return LB_OK;
}

int32_t h_get_compat_facts(void *, LbCompatFacts *out) {
    fill_compat_facts(out);
    return LB_OK;
}

}  // namespace

const LbHostApi &host_api() {
    static const LbHostApi api = [] {
        LbHostApi a{};
        a.struct_size = sizeof(LbHostApi);
        a.abi_version = LB_ABI_VERSION;
        a.ctx = nullptr;
        a.server_print = h_server_print;
        a.client_print = h_client_print;
        a.server_command = h_server_command;
        a.cvar_register = h_cvar_register;
        a.cvar_find = h_cvar_find;
        a.cvar_get_float = h_cvar_get_float;
        a.cvar_get_string = h_cvar_get_string;
        a.cvar_set = h_cvar_set;
        a.trace = h_trace;
        a.trace_batch = h_trace_batch;
        a.point_contents = h_point_contents;
        a.registry_set_rules = h_registry_set_rules;
        a.snapshot_entities = h_snapshot_entities;
        a.get_entity = h_get_entity;
        a.set_capture_mask = h_set_capture_mask;
        a.resolve_user_msg = h_resolve_user_msg;
        a.create_bot = h_create_bot;
        a.kick_bot = h_kick_bot;
        a.bot_client_commands = h_bot_client_commands;
        a.run_player_moves = h_run_player_moves;
        a.get_physics_key = h_get_physics_key;
        a.get_client_info_key = h_get_client_info_key;
        a.get_weapon_data = h_get_weapon_data;
        a.set_bot_disguise = h_set_bot_disguise;
        a.get_player_stats = h_get_player_stats;
        a.load_file = h_load_file;
        a.free_file = h_free_file;
        a.send_debug = h_send_debug;
        a.get_compat_facts = h_get_compat_facts;
        return a;
    }();
    return api;
}

void fill_compat_facts(LbCompatFacts *out) {
    std::memset(out, 0, sizeof(*out));
    auto cvar_str = [](const char *name) -> const char * {
        cvar_t *cv = g_engfuncs.pfnCVarGetPointer(name);
        return cv ? cv->string : nullptr;
    };
    if (cvar_str("host_ver") || cvar_str("build")) {
        out->engine_kind = LB_ENGINE_XASH;
    } else if (cvar_str("sv_rehlds_force_dlmax") || cvar_str("sv_rehlds_hull_centering")) {
        out->engine_kind = LB_ENGINE_REHLDS;
    } else {
        out->engine_kind = LB_ENGINE_HLDS;
    }
    out->metamod_has_hook_tables = state().hooked_dll ? 1 : 0;
    out->channels = LB_CH_AMBIENT_SOUND | LB_CH_PLAYBACK_EVENT;
    if (state().hooked_dll) out->channels |= LB_CH_HOOK_TABLES;
    rehlds_fill_compat(out);
    auto put = [](uint8_t *dst, size_t cap, const char *src) {
        if (src) std::snprintf(reinterpret_cast<char *>(dst), cap, "%s", src);
    };
    put(out->engine_version, sizeof(out->engine_version), cvar_str("host_ver") ? cvar_str("host_ver") : cvar_str("sv_version"));
    put(out->metamod_version, sizeof(out->metamod_version), cvar_str("metamod_version"));
    if (state().gamedll && state().gamedll->dllapi_table && state().gamedll->dllapi_table->pfnGetGameDescription) {
        put(out->gamedll_desc, sizeof(out->gamedll_desc), state().gamedll->dllapi_table->pfnGetGameDescription());
    }
    if (state().util && state().util->get_game_info) {
        put(out->gamedll_path, sizeof(out->gamedll_path), state().util->get_game_info(&state().plugin_info, mm::GINFO_DLL_FULLPATH));
    }
}

}  // namespace lb
