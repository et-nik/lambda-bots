# Corrosion builds the Rust staticlib with cargo and exposes it as a CMake target.
include(FetchContent)
FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.6.1
    GIT_SHALLOW TRUE
)
FetchContent_MakeAvailable(Corrosion)
