# Per-platform output naming and symbol export control for the Metamod module.
function(lb_configure_plugin_target target)
    set_target_properties(${target} PROPERTIES
        PREFIX ""
        CXX_VISIBILITY_PRESET hidden
        VISIBILITY_INLINES_HIDDEN ON)
    set(exports_dir "${PROJECT_SOURCE_DIR}/adapter/exports")
    if(WIN32)
        set_target_properties(${target} PROPERTIES OUTPUT_NAME "lambdabots_mm" MSVC_RUNTIME_LIBRARY "MultiThreaded")
        target_sources(${target} PRIVATE "${exports_dir}/lambdabots.def")
        target_compile_options(${target} PRIVATE /GR- /EHs-c-)
        target_link_options(${target} PRIVATE /SAFESEH:NO)
    elseif(APPLE)
        set_target_properties(${target} PROPERTIES OUTPUT_NAME "lambdabots_mm" SUFFIX ".dylib")
        target_link_options(${target} PRIVATE "-Wl,-exported_symbols_list,${exports_dir}/lambdabots_macos.txt")
        target_compile_options(${target} PRIVATE -fno-exceptions -fno-rtti)
    else()
        if(CMAKE_SIZEOF_VOID_P EQUAL 4)
            set(suffix "_i386")
        elseif(CMAKE_SYSTEM_PROCESSOR MATCHES "aarch64|arm64")
            set(suffix "_arm64")
        else()
            set(suffix "_amd64")
        endif()
        set_target_properties(${target} PROPERTIES OUTPUT_NAME "lambdabots_mm${suffix}" SUFFIX ".so")
        target_compile_options(${target} PRIVATE -fno-exceptions -fno-rtti)
        target_link_options(${target} PRIVATE
            "-Wl,--version-script=${exports_dir}/lambdabots.map"
            -Wl,--exclude-libs,ALL
            -Wl,--no-undefined
            -Wl,--gc-sections
            -static-libstdc++
            -static-libgcc)
    endif()
endfunction()
