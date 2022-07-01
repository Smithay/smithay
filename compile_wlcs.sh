#!/bin/sh

if [ -f "./wlcs/wlcs" ]; then
    echo "Using cached WLCS."
else
    echo "Compiling WLCS."
    git clone https://github.com/MirServer/wlcs.git
    cd wlcs
    # checkout a specific revision
    git reset --hard 22437d7cd78fad156989a8bc9f334ea2be21ce2c
    cmake -DWLCS_BUILD_ASAN=False -DWLCS_BUILD_TSAN=False -DWLCS_BUILD_UBSAN=False .
    make
fi
