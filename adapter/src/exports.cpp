// Metamod plugin entry points and hook tables.
#include <string>

#include "arena.h"
#include "capture.h"
#include "hooks.h"
#include "host_api.h"
#include "registry.h"
#include "rehlds_bridge.h"
#include "lb/platform.h"

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <dlfcn.h>
#endif

namespace lb {

namespace {

mm::PluginInfo make_plugin_info() {
    mm::PluginInfo p{};
    p.ifvers = mm::kInterfaceVersion;
    p.name = "LambdaBots";
    p.version = kAdapterVersion;
    p.date = __DATE__;
    p.author = "lambdabots";
    p.url = "";
    p.logtag = "LB";
    p.loadable = mm::PT_ANYTIME;
    p.unloadable = mm::PT_ANYTIME;
    return p;
}

std::string parent_dir(const std::string &path) {
    const size_t pos = path.find_last_of("/\\");
    return pos == std::string::npos ? std::string() : path.substr(0, pos);
}

std::string base_name(const std::string &path) {
    const size_t pos = path.find_last_of("/\\");
    return pos == std::string::npos ? path : path.substr(pos + 1);
}

// The plugin path as the OS loader sees it (absolute), falling back to Metamod's (relative).
std::string plugin_path() {
#if defined(_WIN32)
    HMODULE module = nullptr;
    if (GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           reinterpret_cast<LPCSTR>(&plugin_path), &module)) {
        char buf[MAX_PATH];
        if (GetModuleFileNameA(module, buf, sizeof(buf))) return buf;
    }
#else
    Dl_info info{};
    if (dladdr(reinterpret_cast<void *>(&plugin_path), &info) && info.dli_fname) return info.dli_fname;
#endif
    if (state().util && state().util->get_plugin_path) {
        const char *p = state().util->get_plugin_path(&state().plugin_info);
        if (p) return p;
    }
    return std::string();
}

void core_init(bool late) {
    char gamedir[512] = {};
    g_engfuncs.pfnGetGameDir(gamedir);
    state().game_dir = gamedir;
    state().plugin_path = plugin_path();
    const std::string bin = parent_dir(state().plugin_path);
    const std::string root = parent_dir(bin);
    if (base_name(bin) == "bin" && !root.empty()) {
        state().install_dir = root;
    } else {
        state().install_dir = std::string(gamedir) + "/addons/lambdabots";
    }

    LbInitInfo info{};
    info.struct_size = sizeof(LbInitInfo);
    info.abi_version = LB_ABI_VERSION;
    info.sizes[LB_SZ_INIT_INFO] = sizeof(LbInitInfo);
    info.sizes[LB_SZ_MAP_INFO] = sizeof(LbMapInfo);
    info.sizes[LB_SZ_FRAME_HEADER] = sizeof(LbFrameHeader);
    info.sizes[LB_SZ_FRAME_INPUT] = sizeof(LbFrameInput);
    info.sizes[LB_SZ_CLIENT_SNAPSHOT] = sizeof(LbClientSnapshot);
    info.sizes[LB_SZ_SELF_SNAPSHOT] = sizeof(LbSelfSnapshot);
    info.sizes[LB_SZ_EVENT_HEADER] = sizeof(LbEventHeader);
    info.sizes[LB_SZ_BOT_COMMAND] = sizeof(LbBotCommand);
    info.sizes[LB_SZ_MOVE_FEEDBACK] = sizeof(LbMoveFeedback);
    info.sizes[LB_SZ_CLIENT_COMMAND] = sizeof(LbClientCommand);
    info.sizes[LB_SZ_TRACE_REQUEST] = sizeof(LbTraceRequest);
    info.sizes[LB_SZ_TRACE_RESULT] = sizeof(LbTraceResult);
    info.sizes[LB_SZ_ENTITY_SNAPSHOT] = sizeof(LbEntitySnapshot);
    info.sizes[LB_SZ_TRACK_RULE] = sizeof(LbTrackRule);
    info.sizes[LB_SZ_CREATE_BOT_REQUEST] = sizeof(LbCreateBotRequest);
    info.sizes[LB_SZ_CREATE_BOT_RESULT] = sizeof(LbCreateBotResult);
    info.sizes[LB_SZ_DEBUG_PRIM] = sizeof(LbDebugPrim);
    info.sizes[LB_SZ_CVAR_SPEC] = sizeof(LbCvarSpec);
    info.sizes[LB_SZ_COMPAT_FACTS] = sizeof(LbCompatFacts);
    info.sizes[LB_SZ_HOST_API] = sizeof(LbHostApi);
    info.sizes[LB_SZ_MSG_ARG] = sizeof(LbMsgArg);
    info.sizes[LB_SZ_DISGUISE] = sizeof(LbDisguise);
    info.adapter_version = str(kAdapterVersion);
    info.plugin_path = str(state().plugin_path);
    info.game_dir = str(state().game_dir);
    info.install_dir = str(state().install_dir);
    info.platform = LB_PLATFORM_ID;
    info.pointer_size = sizeof(void *);
    info.late_load = late ? 1 : 0;

    LbInitResult result{};
    state().core_depth++;
    const int32_t status = lb_core_init(&host_api(), &info, &result);
    state().core_depth--;
    state().core_ok = status == LB_OK;
    if (!state().core_ok) {
        log_console("[lambdabots] core failed to start (status %d); bots are disabled", status);
    }
}

bool map_running() {
    return gpGlobals && gpGlobals->maxClients > 0 && gpGlobals->mapname && lb_string(gpGlobals->mapname)[0] &&
           gpGlobals->time > 0.0f;
}

int get_entity_api2(DLL_FUNCTIONS *table, int *version) {
    if (!table || !version || *version != INTERFACE_VERSION) {
        if (version) *version = INTERFACE_VERSION;
        return FALSE;
    }
    std::memset(table, 0, sizeof(*table));
    table->pfnSpawn = h_Spawn;
    table->pfnClientDisconnect = h_ClientDisconnect;
    table->pfnClientCommand = h_ClientCommand;
    table->pfnServerDeactivate = h_ServerDeactivate;
    table->pfnStartFrame = h_StartFrame;
    table->pfnCmdStart = h_CmdStart;
    table->pfnSys_Error = h_Sys_Error;
    return TRUE;
}

int get_entity_api2_post(DLL_FUNCTIONS *table, int *version) {
    if (!table || !version || *version != INTERFACE_VERSION) {
        if (version) *version = INTERFACE_VERSION;
        return FALSE;
    }
    std::memset(table, 0, sizeof(*table));
    table->pfnSpawn = h_Spawn_Post;
    table->pfnClientConnect = h_ClientConnect_Post;
    table->pfnClientPutInServer = h_ClientPutInServer_Post;
    table->pfnClientUserInfoChanged = h_ClientUserInfoChanged_Post;
    table->pfnServerActivate = h_ServerActivate_Post;
    table->pfnStartFrame = h_StartFrame_Post;
    return TRUE;
}

int get_new_dll_functions(NEW_DLL_FUNCTIONS *table, int *version) {
    if (!table || !version || *version != NEW_DLL_FUNCTIONS_VERSION) {
        if (version) *version = NEW_DLL_FUNCTIONS_VERSION;
        return FALSE;
    }
    std::memset(table, 0, sizeof(*table));
    table->pfnOnFreeEntPrivateData = h_OnFreeEntPrivateData;
    table->pfnGameShutdown = h_GameShutdown;
    return TRUE;
}

int get_engine_functions(enginefuncs_t *table, int *version) {
    if (!table || !version || *version != ENGINE_INTERFACE_VERSION) {
        if (version) *version = ENGINE_INTERFACE_VERSION;
        return FALSE;
    }
    std::memset(table, 0, sizeof(*table));
    table->pfnMessageBegin = e_MessageBegin;
    table->pfnWriteByte = e_WriteByte;
    table->pfnWriteChar = e_WriteChar;
    table->pfnWriteShort = e_WriteShort;
    table->pfnWriteLong = e_WriteLong;
    table->pfnWriteAngle = e_WriteAngle;
    table->pfnWriteCoord = e_WriteCoord;
    table->pfnWriteString = e_WriteString;
    table->pfnWriteEntity = e_WriteEntity;
    table->pfnPlaybackEvent = e_PlaybackEvent;
    table->pfnEmitSound = e_EmitSound;
    table->pfnEmitAmbientSound = e_EmitAmbientSound;
    table->pfnClientCommand = e_ClientCommand;
    table->pfnClientPrintf = e_ClientPrintf;
    table->pfnCmd_Args = e_Cmd_Args;
    table->pfnCmd_Argv = e_Cmd_Argv;
    table->pfnCmd_Argc = e_Cmd_Argc;
    return TRUE;
}

int get_engine_functions_post(enginefuncs_t *table, int *version) {
    if (!table || !version || *version != ENGINE_INTERFACE_VERSION) {
        if (version) *version = ENGINE_INTERFACE_VERSION;
        return FALSE;
    }
    std::memset(table, 0, sizeof(*table));
    table->pfnMessageEnd = e_MessageEnd_Post;
    table->pfnRegUserMsg = e_RegUserMsg_Post;
    table->pfnPrecacheEvent = e_PrecacheEvent_Post;
    return TRUE;
}

}  // namespace

void core_shutdown(uint32_t reason) {
    if (!state().core_ok) return;
    state().core_ok = false;
    state().core_depth++;
    lb_core_shutdown(reason);
    state().core_depth--;
}

}  // namespace lb

using namespace lb;

LB_EXPORT LB_ENTRY void LB_WINAPI GiveFnptrsToDll(enginefuncs_t *engfuncs, globalvars_t *globals) {
    std::memcpy(&g_engfuncs, engfuncs, sizeof(g_engfuncs));
    gpGlobals = globals;
}

LB_EXPORT LB_ENTRY void Meta_Init() {}

LB_EXPORT LB_ENTRY int Meta_Query(const char *interface_version, mm::PluginInfo **plinfo, mm::UtilFuncs *util) {
    state().plugin_info = make_plugin_info();
    *plinfo = &state().plugin_info;
    state().util = util;
    if (interface_version && std::strcmp(interface_version, mm::kInterfaceVersion) != 0) {
        int their_major = 0;
        int their_minor = 0;
        std::sscanf(interface_version, "%d:%d", &their_major, &their_minor);
        if (their_major != 5) {
            log_console("[lambdabots] Metamod interface %s is not supported (need 5:x)", interface_version);
            return FALSE;
        }
    }
    return TRUE;
}

LB_EXPORT LB_ENTRY int Meta_Attach(mm::PlugLoadTime now, mm::MetaFunctions *tables, mm::MetaGlobals *globals,
                                   mm::GameDllFuncs *gamedll) {
    if (!tables || !globals || !gamedll) return FALSE;
    state().meta = globals;
    state().gamedll = gamedll;
    tables->get_entity_api = nullptr;
    tables->get_entity_api_post = nullptr;
    tables->get_entity_api2 = get_entity_api2;
    tables->get_entity_api2_post = get_entity_api2_post;
    tables->get_new_dll_functions = get_new_dll_functions;
    tables->get_new_dll_functions_post = nullptr;
    tables->get_engine_functions = get_engine_functions;
    tables->get_engine_functions_post = get_engine_functions_post;
    if (state().util && state().util->get_hook_tables) {
        state().util->get_hook_tables(&state().plugin_info, &state().hooked_eng, &state().hooked_dll,
                                      &state().hooked_newdll);
    }
    rehlds_init();
    const bool late = now > mm::PT_STARTUP && map_running();
    core_init(late);
    g_engfuncs.pfnAddServerCommand("lb", cmd_lb);
    if (late && state().core_ok) {
        begin_map(true);
        scan_existing_clients();
    }
    log_console("[lambdabots] adapter %s attached%s%s", kAdapterVersion, late ? " (late load)" : "",
                state().hooked_dll ? "" : "; Metamod has no hook tables, other plugins will not see bots");
    return TRUE;
}

LB_EXPORT LB_ENTRY int Meta_Detach(mm::PlugLoadTime, mm::PlUnloadReason) {
    for (int slot = 1; slot <= kMaxSlots && gpGlobals && slot <= gpGlobals->maxClients; slot++) {
        const SlotInfo &s = state().slots[slot];
        if (!s.ours || s.userid <= 0) continue;
        char cmd[64];
        std::snprintf(cmd, sizeof(cmd), "kick #%d\n", s.userid);
        g_engfuncs.pfnServerCommand(cmd);
    }
    core_shutdown(LB_SHUTDOWN_DETACH);
    rehlds_shutdown();
    const uint8_t no_messages[32] = {};
    capture_set_mask(no_messages);
    capture_set_msgmgr_mask(no_messages);
    reset_interned();
    registry_clear();
    arena().clear();
    reset_state_for_reattach();
    return TRUE;
}
