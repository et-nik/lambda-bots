#include "state.h"

#include <chrono>
#include <cstdarg>

enginefuncs_t g_engfuncs;
globalvars_t *gpGlobals = nullptr;

namespace lb {

State &state() {
    static State s;
    return s;
}

void reset_state_for_reattach() {
    State &s = state();
    State fresh;
    fresh.plugin_info = s.plugin_info;
    fresh.util = s.util;
    fresh.bot_gen_counter = s.bot_gen_counter;
    fresh.beam_sprite = s.beam_sprite;
    s = fresh;
}

LbStr str(const char *s) {
    return LbStr{reinterpret_cast<const uint8_t *>(s ? s : ""), static_cast<uint32_t>(s ? std::strlen(s) : 0)};
}

LbStr str(const std::string &s) {
    return LbStr{reinterpret_cast<const uint8_t *>(s.data()), static_cast<uint32_t>(s.size())};
}

void log_console(const char *fmt, ...) {
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    std::vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    if (state().util && state().util->log_console) {
        state().util->log_console(&state().plugin_info, "%s", buf);
    } else if (g_engfuncs.pfnServerPrint) {
        g_engfuncs.pfnServerPrint(buf);
        g_engfuncs.pfnServerPrint("\n");
    }
}

uint64_t mono_ns() {
    using namespace std::chrono;
    return static_cast<uint64_t>(duration_cast<nanoseconds>(steady_clock::now().time_since_epoch()).count());
}

}  // namespace lb
