// Minimal declarations of the ReHLDS public API (github.com/rehlds/ReHLDS, MIT since July 2025), written for
// lambdabots from rehlds/public/rehlds/{rehlds_api,rehlds_interfaces,hookchains,IMessageManager}.h.
//
// Only the entries lambdabots uses are named. The order of virtual functions and struct fields must stay exactly as
// in the upstream headers: a missing or reordered slot silently calls the wrong function. Unused slots are
// declared with placeholder names and `void *` types, which does not change the layout.
#pragma once

#include <cstddef>
#include <cstdint>

struct edict_s;

namespace rehlds {

constexpr const char *kInterfaceVersion = "VREHLDS_HLDS_API_VERSION001";
constexpr int kMajor = 3;
constexpr int kMinMinor = 7;                // GetHostFrameTime
constexpr int kEmitPingsMinor = 11;         // SV_EmitPings hook
constexpr int kMessageManagerMinor = 14;    // GetMessageManager; 3.14 builds older than 830 lack it
constexpr int kMessageManagerBuild = 830;

enum HookChainPriority {
    HC_PRIORITY_UNINTERRUPTABLE = 255,
    HC_PRIORITY_HIGH = 192,
    HC_PRIORITY_DEFAULT = 128,
    HC_PRIORITY_MEDIUM = 64,
    HC_PRIORITY_LOW = 0,
};

template <typename... A>
class IVoidHookChain {
protected:
    virtual ~IVoidHookChain() {}

public:
    virtual void callNext(A... args) = 0;
    virtual void callOriginal(A... args) = 0;
};

template <typename... A>
class IVoidHookChainRegistry {
public:
    typedef void (*hookfunc_t)(IVoidHookChain<A...> *, A...);
    virtual void registerHook(hookfunc_t hook, int priority) = 0;
    virtual void unregisterHook(hookfunc_t hook) = 0;
};

class IGameClient;

using StartSoundChain = IVoidHookChain<int, edict_s *, int, const char *, int, float, int, int>;
using StartSoundRegistry = IVoidHookChainRegistry<int, edict_s *, int, const char *, int, float, int, int>;

class IRehldsHookchains {
public:
    virtual ~IRehldsHookchains() {}
    virtual void *Steam_NotifyClientConnect() = 0;
    virtual void *SV_ConnectClient() = 0;
    virtual void *SV_GetIDString() = 0;
    virtual void *SV_SendServerinfo() = 0;
    virtual void *SV_CheckProtocol() = 0;
    virtual void *SVC_GetChallenge_mod() = 0;
    virtual void *SV_CheckKeyInfo() = 0;
    virtual void *SV_CheckIPRestrictions() = 0;
    virtual void *SV_FinishCertificateCheck() = 0;
    virtual void *Steam_NotifyBotConnect() = 0;
    virtual void *SerializeSteamId() = 0;
    virtual void *SV_CompareUserID() = 0;
    virtual void *Steam_NotifyClientDisconnect() = 0;
    virtual void *PreprocessPacket() = 0;
    virtual void *ValidateCommand() = 0;
    virtual void *ClientConnected() = 0;
    virtual void *HandleNetCommand() = 0;
    virtual void *Mod_LoadBrushModel() = 0;
    virtual void *Mod_LoadStudioModel() = 0;
    virtual void *ExecuteServerStringCmd() = 0;
    virtual void *SV_EmitEvents() = 0;
    virtual void *EV_PlayReliableEvent() = 0;
    virtual StartSoundRegistry *SV_StartSound() = 0;
};

struct RehldsFuncs_t {
    void (*DropClient)(IGameClient *cl, bool crash, const char *fmt, ...);
    void *unused1_16[16];
    int (*GetBuildNumber)();
    double (*GetRealTime)();
    void *unused19_96[78];
    double (*GetHostFrameTime)();
};
static_assert(offsetof(RehldsFuncs_t, GetBuildNumber) == 17 * sizeof(void *), "RehldsFuncs_t layout");
static_assert(offsetof(RehldsFuncs_t, GetHostFrameTime) == 97 * sizeof(void *), "RehldsFuncs_t layout");

class IRehldsServerStatic {
public:
    virtual ~IRehldsServerStatic() {}
    virtual int GetMaxClients() = 0;
    virtual bool IsLogActive() = 0;
    virtual IGameClient *GetClient(int id) = 0;
};

class IRehldsServerData {
public:
    virtual ~IRehldsServerData() {}
    virtual const char *GetModelName() = 0;
    virtual const char *GetName() = 0;
    virtual uint32_t GetWorldmapCrc() = 0;
    virtual uint8_t *GetClientDllMd5() = 0;
    virtual void *GetDatagram() = 0;
    virtual void *GetReliableDatagram() = 0;
    virtual void SetModelName(const char *modelname) = 0;
    virtual void SetConsistencyNum(int num) = 0;
    virtual int GetConsistencyNum() = 0;
    virtual int GetResourcesNum() = 0;
    virtual int GetDecalNameNum() = 0;
    virtual double GetTime() = 0;
};

class IMessage {
public:
    enum class ParamType : uint8_t { Byte, Char, Short, Long, Angle, Coord, String, Entity };
    enum class BlockType : uint8_t { Not, Once, Set };
    enum class Dest : uint8_t { BROADCAST, ONE, ALL, INIT, PVS, PAS, PVS_R, PAS_R, ONE_UNRELIABLE, SPEC };
    enum class DataType : uint8_t { Any, Dest, Index, Origin, Edict, Param, Max };

    virtual ~IMessage() = default;
    virtual int getParamCount() const = 0;
    virtual ParamType getParamType(size_t index) const = 0;
    virtual int getParamInt(size_t index) const = 0;
    virtual float getParamFloat(size_t index) const = 0;
    virtual const char *getParamString(size_t index) const = 0;
    virtual void setParamInt(size_t index, int value) = 0;
    virtual void setParamFloat(size_t index, float value) = 0;
    virtual void setParamVec(size_t index, const float *pos) = 0;
    virtual void setParamString(size_t index, const char *string) = 0;
    virtual Dest getDest() const = 0;
    virtual int getId() const = 0;
    virtual const float *getOrigin() const = 0;
    virtual edict_s *getEdict() const = 0;
};

class IMessageManager {
public:
    using hookfunc_t = void (*)(IVoidHookChain<IMessage *> *chain, IMessage *msg);
    virtual ~IMessageManager() = default;
    virtual int getMajorVersion() const = 0;
    virtual int getMinorVersion() const = 0;
    virtual IMessage::BlockType getMessageBlock(int msg_id) const = 0;
    virtual void setMessageBlock(int msg_id, IMessage::BlockType blockType) = 0;
    virtual void registerHook(int msg_id, hookfunc_t handler, int priority) = 0;
    virtual void unregisterHook(int msg_id, hookfunc_t handler) = 0;
};

class IRehldsApi {
public:
    virtual ~IRehldsApi() {}
    virtual int GetMajorVersion() = 0;
    virtual int GetMinorVersion() = 0;
    virtual const RehldsFuncs_t *GetFuncs() = 0;
    virtual IRehldsHookchains *GetHookchains() = 0;
    virtual IRehldsServerStatic *GetServerStatic() = 0;
    virtual IRehldsServerData *GetServerData() = 0;
    virtual void *GetFlightRecorder() = 0;
    virtual IMessageManager *GetMessageManager() = 0;
};

}  // namespace rehlds
