// Fake clients: creation through the hooked DLL table, kicks, client commands, RunPlayerMove.
#pragma once

#include "state.h"

namespace lb {

int32_t create_bot(const LbCreateBotRequest *req, LbCreateBotResult *out);
int32_t kick_bot(uint8_t slot, uint32_t bot_gen, LbStr reason);
int32_t bot_client_commands(const LbClientCommand *cmds, uint32_t count);
int32_t run_player_moves(const LbBotCommand *cmds, uint32_t count, LbMoveFeedback *feedback);

// Fake argv served through Cmd_Args/Argv/Argc while a bot command executes.
bool fake_argv_active();
int fake_argc();
const char *fake_argv(int i);
const char *fake_args();

// Seed substitution in CmdStart.
bool take_pending_seed(const edict_t *player, uint32_t *seed);

// Network-layer duties the engine skips for fake clients: fixangle, and the fake client flag every spawn clears.
void emulate_network_duties();

void forget_slot(int slot);

}  // namespace lb
