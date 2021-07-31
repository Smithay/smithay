#!/bin/sh

if [ -f "./wlcs/wlcs" ]; then
    echo "Using cached WLCS."
else
    echo "Compiling WLCS."
    git clone https://github.com/MirServer/wlcs.git
    cd wlcs
    # checkout a specific revision
    git reset --hard dcacf09b60f1c5caf103a58046c547df6bb0b85b
    cmake -DWLCS_BUILD_ASAN=False -DWLCS_BUILD_TSAN=False -DWLCS_BUILD_UBSAN=False .
    make
fi
