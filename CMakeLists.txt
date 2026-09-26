cmake_minimum_required(VERSION 3.22)
project(lambdabots VERSION 0.1.0 LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

set(LB_VERSION "${PROJECT_VERSION}" CACHE STRING "Version stamped into the adapter and the core")
if(CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86|i[3-6]86|AMD64|x86_64)$" AND NOT APPLE)
    set(LB_WITH_REHLDS_DEFAULT ON)
else()
    set(LB_WITH_REHLDS_DEFAULT OFF)
endif()
option(LB_WITH_REHLDS "Build the ReHLDS API bridge" ${LB_WITH_REHLDS_DEFAULT})

include(cmake/Corrosion.cmake)
corrosion_import_crate(MANIFEST_PATH Cargo.toml CRATES lb-plugin CRATE_TYPES staticlib)
corrosion_set_env_vars(lb_plugin LB_BUILD_VERSION=${LB_VERSION})
if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
    # The Rust unwinder asks for libgcc_s; -static-libgcc supplies the same symbols from libgcc_eh.a, so the module
    # depends on glibc only.
    get_target_property(lb_rust_native_libs lb_plugin-static INTERFACE_LINK_LIBRARIES)
    list(REMOVE_ITEM lb_rust_native_libs gcc_s)
    set_property(TARGET lb_plugin-static PROPERTY INTERFACE_LINK_LIBRARIES ${lb_rust_native_libs})
endif()

add_library(lambdabots_mm SHARED
    adapter/src/arena.cpp
    adapter/src/capture.cpp
    adapter/src/exports.cpp
    adapter/src/fake_client.cpp
    adapter/src/hooks.cpp
    adapter/src/host_api.cpp
    adapter/src/registry.cpp
    adapter/src/rehlds_bridge.cpp
    adapter/src/state.cpp
)
target_include_directories(lambdabots_mm PRIVATE
    adapter/include
    adapter/include/lb
    third_party/hlsdk/common
    third_party/hlsdk/engine
    third_party/hlsdk/pm_shared
    third_party/hlsdk/dlls
    third_party/hlsdk/public
)
target_compile_definitions(lambdabots_mm PRIVATE LB_ADAPTER_VERSION="${LB_VERSION}")
if(LB_WITH_REHLDS)
    target_compile_definitions(lambdabots_mm PRIVATE LB_WITH_REHLDS=1)
endif()
target_link_libraries(lambdabots_mm PRIVATE lb_plugin)

include(cmake/LbPlatform.cmake)
lb_configure_plugin_target(lambdabots_mm)

set(LB_STAND_DIR "" CACHE PATH "Game directory of a local test server (e.g. .../xash3d-fwgs-apple-arm64/valve)")
if(LB_STAND_DIR)
    add_custom_target(stand-install
        COMMAND ${CMAKE_COMMAND} -E make_directory "${LB_STAND_DIR}/addons/lambdabots/bin"
        COMMAND ${CMAKE_COMMAND} -E copy "$<TARGET_FILE:lambdabots_mm>" "${LB_STAND_DIR}/addons/lambdabots/bin/"
        DEPENDS lambdabots_mm
        COMMENT "Installing lambdabots into ${LB_STAND_DIR}")
endif()

install(TARGETS lambdabots_mm LIBRARY DESTINATION addons/lambdabots/bin RUNTIME DESTINATION addons/lambdabots/bin)
install(DIRECTORY data/ DESTINATION addons/lambdabots)
install(FILES LICENSE NOTICE THIRD_PARTY_LICENSES.md DESTINATION addons/lambdabots OPTIONAL)
