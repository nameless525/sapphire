# The CMake toolchain file a shipping Windows build hands every CMake build of a C dependency (SDL's), through
# CMAKE_TOOLCHAIN_FILE_<target> (M4 X4, xtask/src/platforms/symbols.rs `link_env`). The game links the C
# runtime statically there (`+crt-static`), so a player needs no Visual C++ redistributable. cc-rs passes
# /MT for that on its own; CMake 3.15 and later chooses the runtime by this variable instead, and its default
# is the DLL's (/MD), which would bring VCRUNTIME140.dll back. `package --platform windows` refuses an
# executable that imports a C runtime DLL.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
