// Adapter-wide state: engine/Metamod tables, clocks, slot bookkeeping.
#pragma once

#include <array>
#include <cstdint>
#include <string>

#include "lb/hlsdk.h"
#include "lb/lb_core.h"
#include "lb/metamod_abi.h"

namespace lb {

inline constexpr int kMaxSlots = 64;
inline constexpr const char *kAdapterVersion = LB_ADAPTER_VERSION;

struct SlotInfo {
    bool connected = false;
    bool in_game = false;
    bool ours = false;
    int userid = 0;
    uint32_t bot_gen = 0;
    uint32_t pending_seed = 0;
    bool has_seed = false;
    bool zombie = false;
};

struct State {
    mm::PluginInfo plugin_info{};
    mm::UtilFuncs *util = nullptr;
    mm::MetaGlobals *meta = nullptr;
    mm::GameDllFuncs *gamedll = nullptr;
    enginefuncs_t *hooked_eng = nullptr;
    DLL_FUNCTIONS *hooked_dll = nullptr;
    NEW_DLL_FUNCTIONS *hooked_newdll = nullptr;

    bool core_ok = false;
    int core_depth = 0;
    bool map_active = false;
    bool late_load = false;
    uint32_t epoch = 0;
    uint64_t frame_no = 0;
    bool first_frame = true;
    int max_clients = 0;
    uint32_t bot_gen_counter = 0;
    int creating_slot = -1;
    int beam_sprite = 0;

    // Server clock in double precision: sv.time from ReHLDS, elsewhere the float gpGlobals->time refined by summing
    // frame times (the engine advances sv.time by the previous frame's frametime).
    double sim_time = 0.0;
    double frame_time = 0.0;
    double pending_dt = 0.0;
    bool clock_valid = false;

    std::array<SlotInfo, kMaxSlots + 1> slots{};

    std::string plugin_path;
    std::string game_dir;
    std::string install_dir;
};

State &state();

// Unload leaves this image mapped (Rust registers thread-local destructors, so dlclose keeps it), and a later
// `meta load` attaches to the same statics: forget everything tied to the previous attach.
void reset_state_for_reattach();

inline DLL_FUNCTIONS *dll_for_calls() {
    return state().hooked_dll ? state().hooked_dll : state().gamedll->dllapi_table;
}

inline edict_t *edict_of(int index) {
    return g_engfuncs.pfnPEntityOfEntIndex(index);
}

inline int index_of(const edict_t *ed) {
    return ed ? g_engfuncs.pfnIndexOfEdict(ed) : 0;
}

inline bool is_player_edict(const edict_t *ed) {
    int i = index_of(ed);
    return i >= 1 && i <= state().max_clients;
}

inline bool is_our_bot(const edict_t *ed) {
    int i = index_of(ed);
    return i >= 1 && i <= state().max_clients && i <= kMaxSlots && state().slots[i].ours;
}

LbStr str(const char *s);
LbStr str(const std::string &s);

void log_console(const char *fmt, ...);
uint64_t mono_ns();

}  // namespace lb

#define LB_RETURN_META(result) \
    do {                       \
        lb::state().meta->mres = (result); \
        return;                \
    } while (0)

#define LB_RETURN_META_VALUE(result, value) \
    do {                                    \
        lb::state().meta->mres = (result);  \
        return (value);                     \
    } while (0)
