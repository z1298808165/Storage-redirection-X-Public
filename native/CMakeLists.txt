cmake_minimum_required(VERSION 3.28)
project(srx_lsplant_bridge)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if (NOT DEFINED SRX_INLINE_HOOK_INCLUDE_DIR)
    message(FATAL_ERROR "SRX_INLINE_HOOK_INCLUDE_DIR is required")
endif()

set(PHMAP_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(PHMAP_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(LSPLANT_BUILD_SHARED OFF CACHE BOOL "" FORCE)
set(DEX_BUILDER_BUILD_SHARED OFF CACHE BOOL "" FORCE)
add_subdirectory(${CMAKE_CURRENT_LIST_DIR}/../vendor/lsplant ${CMAKE_BINARY_DIR}/lsplant)

add_library(srx_lsplant_bridge STATIC
    srx_lsplant_bridge.cpp
)
set_target_properties(srx_lsplant_bridge PROPERTIES CXX_SCAN_FOR_MODULES ON)
target_include_directories(srx_lsplant_bridge PRIVATE
    ${CMAKE_CURRENT_LIST_DIR}/../vendor/lsplant/include
    ${SRX_INLINE_HOOK_INCLUDE_DIR}
)
target_link_libraries(srx_lsplant_bridge PRIVATE lsplant_static log)
