# ReHLDS public API: declarations

lambdabots does not copy the ReHLDS SDK: those headers bring their own `const.h`, `edict.h` and typedefs that clash
with hlsdk-portable. `adapter/src/rehlds_api.h` re-declares only the interfaces the adapter calls, keeping the
upstream order of virtual functions and struct fields:

- `IRehldsApi`, `RehldsFuncs_t`, `IRehldsHookchains` (up to `SV_StartSound`);
- `IRehldsServerStatic`, `IRehldsServerData`;
- `IMessage`, `IMessageManager` and the hook chain templates.

Source headers: `rehlds/public/rehlds/{rehlds_api,rehlds_interfaces,hookchains,IMessageManager}.h`.

| Item     | Value                                             |
|----------|---------------------------------------------------|
| Upstream | [rehlds/ReHLDS](https://github.com/rehlds/ReHLDS) |
| Commit   | `6266cd23faee4a6e9cf3974f9605b2cadd86f0a4`        |
| API      | 3.15                                              |
| License  | MIT since July 2025 (`LICENSE`)                   |

Feature gates in the bridge: API 3.7+ for the host frame time, 3.14+ for the message manager.
