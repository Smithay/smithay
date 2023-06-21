#!/bin/sh

WLCS_SHA=34e4804574324fa9f09fe85c19037bcc1444c465

if [ -f "./wlcs/wlcs" ]; then
    echo "Using cached WLCS."
else
    echo "Compiling WLCS."
    git clone https://github.com/MirServer/wlcs.git
    cd wlcs || exit
    # checkout a specific revision
    git reset --hard "${WLCS_SHA}"
    cmake -DWLCS_BUILD_ASAN=False -DWLCS_BUILD_TSAN=False -DWLCS_BUILD_UBSAN=False .
    make
fi
