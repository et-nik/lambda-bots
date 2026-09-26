#pragma once

#include "lb/platform.h"
#include "state.h"

namespace lb {

void begin_map(bool late);
void scan_existing_clients();
void core_shutdown(uint32_t reason);

int h_Spawn(edict_t *ent);
int h_Spawn_Post(edict_t *ent);
qboolean h_ClientConnect_Post(edict_t *ed, const char *name, const char *address, char *reject);
void h_ClientDisconnect(edict_t *ed);
void h_ClientPutInServer_Post(edict_t *ed);
void h_ClientUserInfoChanged_Post(edict_t *ed, char *info);
void h_ClientCommand(edict_t *ed);
void h_ServerActivate_Post(edict_t *list, int edict_count, int client_max);
void h_ServerDeactivate();
void h_StartFrame();
void h_StartFrame_Post();
void h_CmdStart(const edict_t *player, const struct usercmd_s *cmd, unsigned int random_seed);
void h_Sys_Error(const char *message);
void h_OnFreeEntPrivateData(edict_t *ent);
void h_GameShutdown();

void e_MessageBegin(int dest, int type, const float *origin, edict_t *ed);
void e_MessageEnd_Post();
void e_WriteByte(int v);
void e_WriteChar(int v);
void e_WriteShort(int v);
void e_WriteLong(int v);
void e_WriteAngle(float v);
void e_WriteCoord(float v);
void e_WriteString(const char *v);
void e_WriteEntity(int v);
int e_RegUserMsg_Post(const char *name, int size);
unsigned short e_PrecacheEvent_Post(int type, const char *name);
void e_PlaybackEvent(int flags, const edict_t *invoker, unsigned short event_index, float delay, const float *origin,
                     const float *angles, float fparam1, float fparam2, int iparam1, int iparam2, int bparam1,
                     int bparam2);
void e_EmitSound(edict_t *ent, int channel, const char *sample, float volume, float attn, int flags, int pitch);
void e_EmitAmbientSound(edict_t *ent, const float *pos, const char *sample, float volume, float attn, int flags, int pitch);
void e_ClientCommand(edict_t *ed, const char *fmt, ...);
void e_ClientPrintf(edict_t *ed, PRINT_TYPE type, const char *msg);
const char *e_Cmd_Args();
const char *e_Cmd_Argv(int i);
int e_Cmd_Argc();

void cmd_lb();

}  // namespace lb
