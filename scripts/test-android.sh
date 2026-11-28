#!/usr/bin/env bash
# Runs the engine's tests on an Android device, on its GPU and driver:
# builds them for aarch64-linux-android with the NDK's clang, pushes them and
# ImmersiveMap Web's shaders to the device adb sees, runs each one there,
# and fails if a test fails or the shader check skips.
#
#   scripts/test-android.sh
#
# Needs the aarch64-linux-android Rust target, the NDK (ANDROID_NDK_HOME,
# else the newest in $ANDROID_HOME/ndk), one device in `adb devices`, and the
# web's shaders: WEB_SHADERS naming ImmersiveMap Web's
# packages/immersive-map/src, else a checkout of ImmersiveMap Web next to
# this repository.
set -euo pipefail

cd "$(dirname "$0")/../engine"
api=29
remote=/data/local/tmp/immersivemap-engine-tests

ndk="${ANDROID_NDK_HOME:-}"
if [ -z "$ndk" ] && [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME/ndk" ]; then
  ndk="$ANDROID_HOME/ndk/$(ls "$ANDROID_HOME/ndk" | sort -V | tail -1)"
fi
[ -d "$ndk" ] || { echo "No NDK: set ANDROID_NDK_HOME." >&2; exit 1; }
# The NDK's prebuilt toolchain is darwin-x86_64 on every Mac.
host="$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64"
clang="$ndk/toolchains/llvm/prebuilt/$host/bin/aarch64-linux-android$api-clang"
[ -x "$clang" ] || { echo "No $clang." >&2; exit 1; }
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$clang"
export CC_aarch64_linux_android="$clang"
export AR_aarch64_linux_android="$ndk/toolchains/llvm/prebuilt/$host/bin/llvm-ar"

shaders="${WEB_SHADERS:-../../ImmersiveMapWeb/packages/immersive-map/src}"
[ -d "$shaders" ] || { echo "No web shaders at $shaders: set WEB_SHADERS." >&2; exit 1; }

adb get-state >/dev/null || { echo "No device: check 'adb devices'." >&2; exit 1; }

# The test executables, from cargo's own report of what it built.
tests=()
while IFS= read -r executable; do
  tests+=("$executable")
done < <(
  cargo test --no-run --target aarch64-linux-android --message-format=json 2>/dev/null |
    grep '"reason":"compiler-artifact"' | grep '"test":true' |
    sed -n 's/.*"executable":"\([^"]*\)".*/\1/p'
)
[ "${#tests[@]}" -gt 0 ] || { echo "No test was built." >&2; exit 1; }
echo "${#tests[@]} test binaries"

# The shaders alone, in their directories: the requires rule reads next to each file.
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
(cd "$shaders" && find . -name '*.wgsl' -exec cp --parents {} "$staging/" \;)
adb shell "rm -rf $remote && mkdir -p $remote/shaders"
adb push "${tests[@]}" "$remote/" >/dev/null
adb push "$staging/." "$remote/shaders/" >/dev/null

passed=0
failed=()
skipped=()
for test in "${tests[@]}"; do
  name=$(basename "$test")
  output=$(adb shell "cd $remote && WEB_SHADERS=$remote/shaders ./$name --nocapture 2>&1; echo EXIT=\$?")
  summary=$(echo "$output" | grep '^test result' || true)
  echo "$name: ${summary:-no result}"
  echo "$output" | grep -E '^adapter:|^features|^limits:|pipelines built|^skipped:|^FAIL' || true
  if echo "$output" | grep -q 'EXIT=0'; then
    passed=$((passed + $(echo "$summary" | sed -n 's/.* \([0-9][0-9]*\) passed.*/\1/p')))
  else
    failed+=("$name")
    echo "$output" | grep -E 'FAILED|panicked' | head -5
  fi
  if echo "$output" | grep -qE 'shader check is skipped|frame tests are skipped'; then
    skipped+=("$name")
  fi
done

echo
echo "$passed tests passed on $(adb shell getprop ro.product.model | tr -d '\r') (Android $(adb shell getprop ro.build.version.release | tr -d '\r'))."
if [ "${#skipped[@]}" -gt 0 ]; then
  echo "Skipped on the device: ${skipped[*]}" >&2
fi
if [ "${#failed[@]}" -gt 0 ]; then
  echo "Failed: ${failed[*]}" >&2
fi
[ "${#failed[@]}" -eq 0 ] && [ "${#skipped[@]}" -eq 0 ]
