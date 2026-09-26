// Per-frame event arena: hooks append typed records, StartFrame hands the filled buffer to the core.
#pragma once

#include <cstdint>
#include <initializer_list>
#include <utility>
#include <vector>

#include "lb/lb_abi.h"

namespace lb {

struct Part {
    const void *data;
    size_t size;
};

class Arena {
public:
    Arena();

    // Appends one record. Non-critical records are dropped once the non-critical budget is used.
    bool push(uint16_t kind, bool critical, std::initializer_list<Part> parts);

    // Moves the recorded events to the delivery buffer and starts a new recording buffer.
    void swap();

    // Drops everything recorded so far (plugin unload: the next core instance starts clean).
    void clear();

    const std::vector<uint8_t> &delivered() const { return front_; }
    uint32_t delivered_count() const { return front_count_; }
    uint32_t delivered_dropped() const { return front_dropped_; }
    uint32_t delivered_first_seq() const { return front_first_seq_; }

    void set_context(uint32_t ctx) { ctx_ = ctx; }
    uint32_t context() const { return ctx_; }

private:
    std::vector<uint8_t> back_;
    std::vector<uint8_t> front_;
    uint32_t back_count_ = 0;
    uint32_t back_dropped_ = 0;
    uint32_t back_dropped_bytes_ = 0;
    uint32_t back_first_seq_ = 0;
    uint32_t front_count_ = 0;
    uint32_t front_dropped_ = 0;
    uint32_t front_first_seq_ = 0;
    uint32_t seq_ = 1;
    uint32_t ctx_ = LB_CTX_FRAME;
};

Arena &arena();

// RAII context marker for records captured while we execute a bot command.
class ArenaContext {
public:
    explicit ArenaContext(uint32_t ctx) : prev_(arena().context()) { arena().set_context(ctx); }
    ~ArenaContext() { arena().set_context(prev_); }
    ArenaContext(const ArenaContext &) = delete;
    ArenaContext &operator=(const ArenaContext &) = delete;

private:
    uint32_t prev_;
};

}  // namespace lb
