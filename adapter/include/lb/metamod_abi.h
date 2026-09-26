// Metamod plugin interface "5:13" as seen by a plugin, declared independently for lambdabots
// (MIT). Layouts are verified against metamod-fwgs headers in CI; no metamod code is included.
#pragma once

#include "hlsdk.h"

namespace mm {

inline constexpr const char *kInterfaceVersion = "5:13";

enum PlugLoadTime { PT_NEVER = 0, PT_STARTUP, PT_CHANGELEVEL, PT_ANYTIME, PT_ANYPAUSE };

enum PlUnloadReason {
    PNL_NULL = 0,
    PNL_INI_DELETED,
    PNL_FILE_NEWER,
    PNL_COMMAND,
    PNL_CMD_FORCED,
    PNL_DELAYED,
    PNL_PLUGIN,
    PNL_PLG_FORCED,
    PNL_RELOAD,
};

enum MetaRes { MRES_UNSET = 0, MRES_IGNORED, MRES_HANDLED, MRES_OVERRIDE, MRES_SUPERCEDE };

enum GameInfoTag { GINFO_NAME = 0, GINFO_DESC, GINFO_GAMEDIR, GINFO_DLL_FULLPATH, GINFO_DLL_FILENAME, GINFO_REALDLL_FULLPATH };

struct PluginInfo {
    const char *ifvers;
    const char *name;
    const char *version;
    const char *date;
    const char *author;
    const char *url;
    const char *logtag;
    PlugLoadTime loadable;
    PlugLoadTime unloadable;
};

using Plid = PluginInfo *;

struct MetaGlobals {
    MetaRes mres;
    MetaRes prev_mres;
    MetaRes status;
    void *orig_ret;
    void *override_ret;
};

using GetEntityApiFn = int (*)(DLL_FUNCTIONS *table, int interface_version);
using GetEntityApi2Fn = int (*)(DLL_FUNCTIONS *table, int *interface_version);
using GetNewDllFunctionsFn = int (*)(NEW_DLL_FUNCTIONS *table, int *interface_version);
using GetEngineFunctionsFn = int (*)(enginefuncs_t *table, int *interface_version);

struct MetaFunctions {
    GetEntityApiFn get_entity_api;
    GetEntityApiFn get_entity_api_post;
    GetEntityApi2Fn get_entity_api2;
    GetEntityApi2Fn get_entity_api2_post;
    GetNewDllFunctionsFn get_new_dll_functions;
    GetNewDllFunctionsFn get_new_dll_functions_post;
    GetEngineFunctionsFn get_engine_functions;
    GetEngineFunctionsFn get_engine_functions_post;
};

struct GameDllFuncs {
    DLL_FUNCTIONS *dllapi_table;
    NEW_DLL_FUNCTIONS *newapi_table;
};

struct UtilFuncs {
    void (*log_console)(Plid plid, const char *fmt, ...);
    void (*log_message)(Plid plid, const char *fmt, ...);
    void (*log_error)(Plid plid, const char *fmt, ...);
    void (*log_developer)(Plid plid, const char *fmt, ...);
    void (*center_say)(Plid plid, const char *fmt, ...);
    void *center_say_parms;
    void *center_say_varargs;
    qboolean (*call_game_entity)(Plid plid, const char *ent_str, entvars_t *pev);
    int (*get_user_msg_id)(Plid plid, const char *msgname, int *size);
    const char *(*get_user_msg_name)(Plid plid, int msgid, int *size);
    const char *(*get_plugin_path)(Plid plid);
    const char *(*get_game_info)(Plid plid, GameInfoTag tag);
    void *load_plugin;
    void *unload_plugin;
    void *unload_plugin_by_handle;
    const char *(*is_querying_client_cvar)(Plid plid, const edict_t *ed);
    int (*make_request_id)(Plid plid);
    void (*get_hook_tables)(Plid plid, enginefuncs_t **peng, DLL_FUNCTIONS **pdll, NEW_DLL_FUNCTIONS **pnewdll);
};

}  // namespace mm
