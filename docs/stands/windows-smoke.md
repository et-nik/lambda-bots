# Windows smoke test

The Windows build is not deployed to production, but it must load and work. The check is manual and takes 10 minutes.

## Module

`lambdabots_mm.dll` is taken from the CI artifact (`windows-x86`) or built locally from the x86 Developer PowerShell
for Visual Studio 2022:

```powershell
scripts/build-windows.ps1
```

The script already checks that there are exactly five exports and that the CRT is static. To repeat manually:

```powershell
dumpbin /exports build\release-windows-x86\Release\lambdabots_mm.dll
dumpbin /dependents build\release-windows-x86\Release\lambdabots_mm.dll
```

| Check      | Expected                                                                   |
|------------|----------------------------------------------------------------------------|
| exports    | `GiveFnptrsToDll`, `Meta_Attach`, `Meta_Detach`, `Meta_Init`, `Meta_Query` |
| dependents | system DLLs only (`KERNEL32`, `ADVAPI32`, `WS2_32`, `bcrypt`, `ntdll` …)   |
| not listed | `MSVCP*.dll`, `VCRUNTIME*.dll`, `api-ms-win-crt-*`                         |

## Server

1. HLDS (steamcmd, `app_update 90`) with ReHLDS and Metamod-r, or plain HLDS with Metamod-r.
2. Extract the `lambdabots-<version>-windows-x86.zip` archive into `valve\`.
3. In `valve\addons\metamod\plugins.ini`: `win32 addons/lambdabots/bin/lambdabots_mm.dll`.
4. Start: `hlds.exe -console -game valve +maxplayers 16 +sys_ticrate 1000 +map crossfire`.

## Scenario

| Command                                             | Expected                                                           |
|-----------------------------------------------------|--------------------------------------------------------------------|
| `meta list`                                         | LambdaBots in RUN status                                           |
| `lb compat`                                         | `platform: windows`, correct engine version, `metamod_hook_tables` |
| wait 20 s, `lb list`                                | 8 bots in the alive state                                          |
| `lb kick #2`, `lb list`                             | the bot left, quota 7, no new bot joins                            |
| `lb add 2`, after 10 s `lb list`                    | 9 bots                                                             |
| `lb kill all`, after 4 s `lb list`                  | all alive again                                                    |
| `changelevel stalkyard`                             | the same bot names after loading                                   |
| `meta unload lambdabots`                            | bots kicked, server keeps running                                  |
| `meta load addons/lambdabots/bin/lambdabots_mm.dll` | the module loads, bots join again                                  |

Run results — engine, Metamod and module versions, deviations from the table — go into an issue or the release notes.
