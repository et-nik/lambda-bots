// Capture of engine traffic into the event arena: user messages, sounds, events, clients, strings.
#pragma once

#include <cstdint>
#include <string>

#include "state.h"

namespace lb {

// User messages, as Metamod shows them: what the game DLL sends (other plugins call the raw engine functions).
void capture_set_mask(const uint8_t mask[32]);
void capture_message_begin(int dest, int type, const float *origin, edict_t *ed);
void capture_arg_int(uint8_t tag, int value);
void capture_arg_float(uint8_t tag, float value);
void capture_arg_string(const char *value);
void capture_message_end();
void capture_set_own_send(bool own);

// Names announced by the engine.
void record_reg_msg(int id, const char *name);
void record_precache_event(int index, const char *name);

// Strings interned for snapshots (model names); announced once per map.
uint16_t intern(const char *s);
void reset_interned();

// Clients, commands, sounds, events, entities, logs.
void record_client(uint8_t what, int slot, const char *name, const char *model, const char *addr, const char *auth);
void record_command(uint16_t kind, int slot, int userid, int argc, const char *const *argv, const char *line);
void record_sound(uint8_t source, edict_t *ent, int channel, const char *sample, const float *origin, float volume,
                  float attn, int flags, int pitch);
void record_playback(int flags, const edict_t *invoker, unsigned short event_index, float delay, const float *origin,
                     const float *angles, float fparam1, float fparam2, int iparam1, int iparam2, int bparam1,
                     int bparam2);
void record_entity(uint8_t what, uint8_t kind, edict_t *ent);
void record_fixangle(int slot, uint32_t bot_gen, uint8_t mode, const float *angles);
void record_log(int level, const std::string &text);

LbEntRef ent_ref(const edict_t *ed);

}  // namespace lb
