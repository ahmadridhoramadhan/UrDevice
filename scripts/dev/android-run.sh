#!/usr/bin/env bash
# Build the debug APK, install it on the USB phone, and start the app.
#
#   scripts/dev/android-run.sh [abi]     (default: arm64-v8a; "all" builds every ABI)
#
# Set ANDROID_SERIAL to pick a device when more than one is attached.
source "$(dirname "$0")/lib.sh"

abi="${1:-arm64-v8a}"
abi_flag=()
[[ "$abi" != "all" ]] && abi_flag=("-Purdevice.abis=$abi")

build() {
    (cd "$REPO_ROOT/android" && GRADLE_OPTS="${GRADLE_OPTS:--Xmx1536m}" ./gradlew assembleDebug "${abi_flag[@]}" --console=plain -q)
}
install() {
    adb install -r "$REPO_ROOT/android/app/build/outputs/apk/debug/app-debug.apk"
    adb shell am start -n com.urdevice.app/.MainActivity
}
with_lock gradle build
with_lock android install
