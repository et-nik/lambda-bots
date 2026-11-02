// `lb_aimlog on|off`: how people aim, written to study it. Every usercmd of a human player (its msec, view angles and
// buttons), the players' state up to 250 times a second (bots too, with their own view angles), when another player
// comes into and goes out of a human's sight, and the weapon events. One tab-separated file a map:
// logs/aim-<map>-<date>.tsv. It lives in the adapter, outside the core: no bot ever sees any of it.
#pragma once

#include "lb/platform.h"
#include "state.h"

struct usercmd_s;

namespace lb {

LB_ENTRY void aimlog_command();
void aimlog_map_start();
void aimlog_map_end();
void aimlog_frame();
void aimlog_cmd(const edict_t *player, const usercmd_s *cmd);
void aimlog_event_name(unsigned short index, const char *name);
void aimlog_playback(const edict_t *invoker, unsigned short event_index);

}  // namespace lb
