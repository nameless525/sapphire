package dev.sapphire.sandbox;

import org.libsdl.app.SDLActivity;

/**
 * Hosts the sandbox. SDL's Java glue drives the Rust app in {@code libsapphire_sandbox.so},
 * calling its exported {@code SDL_main} on SDL's thread (see the "Android packaging" docs of
 * the {@code sapphire_platform} crate).
 */
public final class SandboxActivity extends SDLActivity {
    /**
     * String-array extra with the app's command-line arguments, which
     * {@code cargo xtask run --platform android -- <args>} sets through {@code am start --esa}.
     */
    static final String EXTRA_ARGS = "sapphire.args";

    @Override
    protected String[] getLibraries() {
        // SDL and libc++ are linked statically into the app library: there is no libSDL3.so.
        return new String[] {"sapphire_sandbox"};
    }

    @Override
    protected String[] getArguments() {
        String[] args = getIntent().getStringArrayExtra(EXTRA_ARGS);
        if (args == null) {
            return new String[0];
        }
        // `am start --esa` splits on commas but leaves escaped ones (`\,`) escaped.
        for (int i = 0; i < args.length; i++) {
            args[i] = args[i].replace("\\,", ",");
        }
        return args;
    }
}
