#!/bin/sh

if [ -f "./wlcs/wlcs" ]; then
    echo "Using cached WLCS."
else
    echo "Compiling WLCS."
    git clone https://github.com/MirServer/wlcs.git
    cd wlcs
    # checkout a specific revision
    git reset --hard dbee179e91a140d0725e15ba60ea12f6c89d0904
    cmake -DWLCS_BUILD_ASAN=False -DWLCS_BUILD_TSAN=False -DWLCS_BUILD_UBSAN=False .
    make
fi
