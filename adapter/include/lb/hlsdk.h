// Half-Life SDK types used by the adapter (vendored hlsdk-portable headers, 64-bit clean).
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>

#include "mathlib.h"
#include "const.h"
#include "progdefs.h"
#include "edict.h"
#include "eiface.h"
#include "cdll_dll.h"

#ifndef TRUE
#define TRUE 1
#endif
#ifndef FALSE
#define FALSE 0
#endif
#ifndef SVC_TEMPENTITY
#define SVC_TEMPENTITY 23
#endif
#ifndef SVC_INTERMISSION
#define SVC_INTERMISSION 30
#endif

// Engine function table version expected by Metamod (engine_api.h in metamod).
inline constexpr int ENGINE_INTERFACE_VERSION = 138;

extern enginefuncs_t g_engfuncs;
extern globalvars_t *gpGlobals;

inline const char *lb_string(string_t s) {
    return gpGlobals->pStringBase + s;
}
