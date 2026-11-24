# ImmersiveMap Android

[![License](https://img.shields.io/github/license/artembobkin/ImmersiveMapAndroid?style=flat-square)](LICENSE)

ImmersiveMap Android is the Android port of [ImmersiveMap](https://github.com/artembobkin/ImmersiveMap), the Swift and Metal map engine for Apple platforms. The engine is Rust, drawing with [wgpu](https://wgpu.rs) over Vulkan, and the library an app adds is Kotlin with a Jetpack Compose entry.

It shares its engine code with [ImmersiveMap Web](https://github.com/artembobkin/ImmersiveMapWeb) through [ImmersiveMapCore](https://github.com/artembobkin/ImmersiveMapCore): the camera, the presentation, the tile coverage, the tile parse and the style are the same Rust on both.

## State

This is the start of the port. What works:

- The camera of the shared core: drag, pinch about a point, two-finger turn, tilt, the zoom and pitch ranges.
- The frame's view of the shared core, in one call a frame: presentation, render camera, tile coverage with its ring rules.
- A renderer that draws the coverage itself, each tile a square coloured by its zoom, so the bands the map will be drawn in can be seen and moved through.
- Rendering on demand: a frame is drawn at the next vsync after something asks for one, and nothing is drawn while nothing moves.

What is not there yet: loading tiles and drawing their contents, labels, the globe (the map is the plane at every zoom), buildings, the relief, everything else the other ports draw.

It has been built and its engine tested on a Mac. It has not been run on an Android device or emulator yet.

## Layout

- `engine/`: the engine, a Rust crate. `src/engine.rs` is free of the platform: it draws into any wgpu surface, or into a texture, so it is tested on the machine it is written on. `src/android.rs` is the JNI binding.
- `immersivemap/`: the Android library. `ImmersiveMapView` owns the `Surface`, the render thread and the gestures, and `ImmersiveMap` is its Compose entry. Its build compiles the engine with cargo-ndk.
- `app/`: the app the work in progress is run in.

## Building

Needed once:

- A JDK 17 or newer, and the Android SDK with platform 37.2 and NDK 28.2.13676358 (`sdkmanager "platforms;android-37.2" "ndk;28.2.13676358"`). Point `ANDROID_HOME` at the SDK or write `sdk.dir` in `local.properties`.
- Rust with the Android targets and cargo-ndk:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
```

Then:

```sh
./gradlew :app:assembleDebug      # the engine for arm64-v8a and x86_64, the library and the app
./gradlew :app:installDebug       # onto a connected device or a running emulator
```

Android Studio opens the project as it is.

## Testing the engine

The engine's tests draw frames to a texture on the host's GPU, inside a validation scope, and drive the gestures:

```sh
cd engine
cargo test
cargo clippy --all-targets -- -D warnings
cargo ndk -t arm64-v8a clippy -- -D warnings      # the Android binding, with ANDROID_NDK_HOME set
```

## Using it

```kotlin
val map = rememberImmersiveMapState()
ImmersiveMap(Modifier.fillMaxSize(), map)
// Latitude and longitude in degrees, the bearing and the pitch in radians.
map.setCameraPosition(40.7128, -74.0060, zoom = 14.0, bearing = 0.4, pitch = 1.0)
```

Android 10 (API 29) and newer, 64-bit devices.

## License

MIT, see [LICENSE](LICENSE).
