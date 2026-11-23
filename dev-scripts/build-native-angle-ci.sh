#!/usr/bin/env bash
# Build only on GitHub-hosted CI. Mirrors angle-android's dependency bootstrap.
set -euo pipefail
if [[ "${GITHUB_ACTIONS:-}" != true ]]; then
    echo "This diagnostic build must run through GitHub Actions." >&2
    exit 1
fi
exec > >(tee native-angle-build.log) 2>&1
angle_commit=b7a90a6cc08e52857a6addc2bc2e569b7b01e9df
git clone --depth=1 https://chromium.googlesource.com/chromium/tools/depot_tools.git native-depot-tools
export PATH="$PWD/native-depot-tools:$PATH"
export VPYTHON_BYPASS="manually managed python not supported by chrome operations"
# Use a current prebuilt ARM GN; Ubuntu's package predates allowlists.
curl -fsSL https://chrome-infra-packages.appspot.com/dl/gn/gn/linux-arm64/+/latest -o native-gn.zip
mkdir native-gn
unzip -q native-gn.zip -d native-gn
chmod +x native-gn/gn
export PATH="$PWD/native-gn:$PATH"
mkdir native-angle
cd native-angle
git init -q
git remote add origin https://chromium.googlesource.com/angle/angle
git fetch --depth=1 origin "$angle_commit"
git checkout --detach FETCH_HEAD
python3 scripts/bootstrap.py
# These are test/source dependencies, not the GLES/Vulkan renderer.
sed -i -e "/'third_party\/dawn'\: /,+3d" -e "/'third_party\/llvm\/src'\: /,+3d" -e "/'third_party\/SwiftShader'\: /,+3d" -e "/'third_party\/VK-GL-CTS\/src'\: /,+3d" DEPS
# The bundled tool hooks download x86_64 Rust/Clang/GN on Linux. Use the
# ARM runner's compiler and GN/Ninja; these libraries do not need Rust.
gclient sync --nohooks --no-history -f -D -R
python3 build/util/lastchange.py -o build/util/LASTCHANGE
mkdir -p out/NativeGLES
cat > out/NativeGLES/args.gn <<'EOF'
target_os = "linux"
target_cpu = "arm64"
is_debug = false
is_component_build = false
is_clang = false
symbol_level = 0
angle_build_all = false
angle_build_tests = false
angle_enable_gl = true
angle_enable_vulkan = true
angle_enable_swiftshader = false
angle_enable_d3d9 = false
angle_enable_d3d11 = false
angle_enable_metal = false
angle_enable_wgpu = false
angle_enable_null = false
angle_enable_glsl = true
angle_enable_essl = true
angle_enable_abseil = false
angle_enable_share_context_lock = false
angle_has_frame_capture = false
use_custom_libcxx = false
use_sysroot = false
use_siso = false
use_thin_lto = false
enable_rust = false
treat_warnings_as_errors = false
EOF
gn gen out/NativeGLES
/usr/bin/ninja -C out/NativeGLES libEGL libGLESv2 libGLESv1_CM
mkdir -p ../native-angle-bundle
cp out/NativeGLES/libEGL.so out/NativeGLES/libGLESv2.so out/NativeGLES/libGLESv1_CM.so ../native-angle-bundle/
printf '%s\n' "$angle_commit" > ../native-angle-bundle/commit.txt
