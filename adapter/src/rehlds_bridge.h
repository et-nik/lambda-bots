// Optional ReHLDS channels. Without ReHLDS (or when built without LB_WITH_REHLDS) every query answers "not
// available" and the adapter falls back to Metamod hooks and server commands.
#pragma once

#include "state.h"

namespace lb {

bool rehlds_init();
void rehlds_shutdown();
void rehlds_fill_compat(LbCompatFacts *out);

// SV_StartSound is hooked: every sound (including footsteps from player movement) arrives through the bridge.
bool rehlds_sound_channel();

// Drops a client immediately (instead of a deferred `kick`).
bool rehlds_drop_client(int slot, const char *reason);

// sv.time and host_frametime in double precision.
bool rehlds_time(double *sim_time, double *frame_time);

}  // namespace lb
