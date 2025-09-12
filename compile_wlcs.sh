#!/bin/sh

WLCS_SHA=2b385328366de5db19521b9b3eef81861430d984

if [ -f "./wlcs/wlcs" ] && [ "$(cd wlcs; git rev-parse HEAD)" = "${WLCS_SHA}" ] ; then
    echo "Using cached WLCS."
else
    echo "Compiling WLCS."
    git clone https://github.com/MirServer/wlcs.git
    cd wlcs || exit
    # checkout a specific revision
    git reset --hard "${WLCS_SHA}"
    cmake -DWLCS_BUILD_ASAN=False -DWLCS_BUILD_TSAN=False -DWLCS_BUILD_UBSAN=False -DCMAKE_EXPORT_COMPILE_COMMANDS=1 .
    make
fi
