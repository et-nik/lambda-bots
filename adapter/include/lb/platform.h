// Platform macros for functions called by the engine or Metamod.
#pragma once

#if defined(_WIN32)
// Exported by adapter/exports/lambdabots.def alone: __declspec(dllexport) would add the decorated _GiveFnptrsToDll@8.
#define LB_EXPORT extern "C"
#define LB_WINAPI __stdcall
#else
#define LB_EXPORT extern "C" __attribute__((visibility("default")))
#define LB_WINAPI
#endif

// The engine may call us with a 4-byte aligned stack on i386; realign for SSE code.
#if defined(__i386__) && (defined(__GNUC__) || defined(__clang__))
#define LB_ENTRY __attribute__((force_align_arg_pointer))
#else
#define LB_ENTRY
#endif

#if defined(__linux__)
#define LB_PLATFORM_ID 1
#elif defined(_WIN32)
#define LB_PLATFORM_ID 2
#elif defined(__APPLE__)
#define LB_PLATFORM_ID 3
#else
#define LB_PLATFORM_ID 0
#endif
