package dev.sapphire.sandbox;

import android.content.pm.ApplicationInfo;
import android.util.Base64;
import android.util.Log;
import java.nio.charset.StandardCharsets;
import org.json.JSONArray;
import org.json.JSONException;
import org.libsdl.app.SDLActivity;

/**
 * Hosts the sandbox. SDL's Java glue drives the Rust app in {@code libsapphire_sandbox.so},
 * calling its exported {@code SDL_main} on SDL's thread (see the "Android packaging" docs of
 * the {@code sapphire_platform} crate).
 */
public final class SandboxActivity extends SDLActivity {
    /**
     * String extra with the app's command-line arguments: a base64-encoded JSON array of
     * strings, which {@code cargo xtask run --platform android -- <args>} sets. Base64 keeps
     * the value clear of the device shell's and {@code am}'s quoting rules.
     */
    static final String EXTRA_ARGS = "sapphire.args";

    private static final String TAG = "Sapphire";

    @Override
    protected String[] getLibraries() {
        // SDL and libc++ are linked statically into the app library: there is no libSDL3.so.
        return new String[] {"sapphire_sandbox"};
    }

    @Override
    protected String[] getArguments() {
        String encoded = getIntent().getStringExtra(EXTRA_ARGS);
        if (encoded == null) {
            return new String[0];
        }
        // The activity is exported, so any app can start it with extras. Only development
        // (debuggable) builds take arguments from them.
        if ((getApplicationInfo().flags & ApplicationInfo.FLAG_DEBUGGABLE) == 0) {
            Log.w(TAG, "ignoring launch arguments: this build is not debuggable");
            return new String[0];
        }
        try {
            String json = new String(Base64.decode(encoded, Base64.DEFAULT), StandardCharsets.UTF_8);
            JSONArray array = new JSONArray(json);
            String[] args = new String[array.length()];
            for (int i = 0; i < args.length; i++) {
                args[i] = array.getString(i);
            }
            return args;
        } catch (IllegalArgumentException | JSONException e) {
            Log.e(TAG, "ignoring malformed " + EXTRA_ARGS + " extra", e);
            return new String[0];
        }
    }
}
