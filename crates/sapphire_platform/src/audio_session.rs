//! The OS's side of playing sound (M2 E1, spec §5.15): what an app does once, before its first
//! audio stream, and what the OS says afterwards.
//!
//! - **iOS:** the audio session's category and activation, and its interruptions (calls, Siri,
//!   alarms) as a flag the app polls. Without a category an app takes the default, which stops the
//!   player's music.
//! - **Android:** cpal's AAudio host reads the app's `JavaVM` and activity through `ndk-context`
//!   when it opens a stream, and panics if nobody set them. `cargo-apk`'s glue would; SDL's entry
//!   doesn't, so this does, from SDL's own JNI handles.
//! - **Desktop:** nothing.
//!
//! Only apps that play sound come here, so an app that plays none has no session and takes no
//! audio focus.

/// The session and context for this OS, once per process. On the main thread (SDL's thread on
/// Android), where SDL's JNI environment and UIKit's calls are valid.
pub(crate) fn prepare() -> Result<(), String> {
    imp::prepare()
}

/// Whether the OS has taken audio away, as far as the session says.
pub(crate) fn interrupted() -> bool {
    imp::interrupted()
}

/// Asks for audio again, coming back to the foreground. Returns whether the OS gave it.
pub(crate) fn activate() -> bool {
    imp::activate()
}

#[cfg(target_os = "ios")]
mod imp {
    use std::ptr::NonNull;
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionCategoryAmbient, AVAudioSessionInterruptionNotification,
        AVAudioSessionInterruptionType, AVAudioSessionInterruptionTypeKey,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSNumber};

    use crate::LOG_TARGET;

    /// The hardware buffer to ask for, in seconds: a hit should be heard within a frame or two, and
    /// iOS's default is about 23 ms. It is only a request, and costs a little more battery.
    const PREFERRED_IO_BUFFER_SECONDS: f64 = 0.010;

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);
    static OBSERVE: Once = Once::new();

    pub(super) fn prepare() -> Result<(), String> {
        // SAFETY: the shared session may be used from any thread; `Ambient` is a valid category,
        // which lets the player's music play on under the game and follows the silent switch.
        unsafe {
            let session = AVAudioSession::sharedInstance();
            let category = AVAudioSessionCategoryAmbient.ok_or("iOS has no ambient audio category")?;
            session.setCategory_error(category).map_err(|error| format!("setting the category: {error}"))?;
            if let Err(error) = session.setPreferredIOBufferDuration_error(PREFERRED_IO_BUFFER_SECONDS) {
                tracing::warn!(target: LOG_TARGET, %error, "the audio session refused a smaller buffer");
            }
            session.setActive_error(true).map_err(|error| format!("activating: {error}"))?;
        }
        OBSERVE.call_once(observe_interruptions);
        Ok(())
    }

    pub(super) fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }

    pub(super) fn activate() -> bool {
        // SAFETY: as in `prepare`.
        let activated = unsafe { AVAudioSession::sharedInstance().setActive_error(true) };
        match activated {
            Ok(()) => {
                INTERRUPTED.store(false, Ordering::Relaxed);
                true
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "the audio session stays inactive");
                false
            }
        }
    }

    /// Sets [`INTERRUPTED`] from the session's notifications, for the rest of the process. cpal has
    /// its own observer, which stops and restarts its audio unit, but only when the OS suggests a
    /// restart. An interruption's end without that suggestion is an app's to answer, so the flag
    /// clears either way, once the session reactivates; the app asks the mixer to go on.
    fn observe_interruptions() {
        let block = RcBlock::new(|notification: NonNull<NSNotification>| {
            // SAFETY: the center passes a valid notification for the block's duration.
            let notification = unsafe { notification.as_ref() };
            // SAFETY: a constant AVFAudio exports.
            let key = unsafe { AVAudioSessionInterruptionTypeKey };
            let kind = notification
                .userInfo()
                .zip(key)
                .and_then(|(info, key)| info.objectForKey(key))
                .and_then(|value| value.downcast::<NSNumber>().ok())
                .map(|number| number.unsignedIntegerValue());
            let Some(kind) = kind else { return };
            let began = AVAudioSessionInterruptionType(kind) == AVAudioSessionInterruptionType::Began;
            tracing::info!(target: LOG_TARGET, began, "audio session interruption");
            if began {
                INTERRUPTED.store(true, Ordering::Relaxed);
            } else {
                activate();
            }
        });
        // SAFETY: a constant AVFAudio exports. The center copies the block; the observer token is
        // kept for the life of the process, so it is never removed.
        unsafe {
            if let Some(name) = AVAudioSessionInterruptionNotification {
                let observer = NSNotificationCenter::defaultCenter()
                    .addObserverForName_object_queue_usingBlock(Some(name), None, None, &block);
                std::mem::forget(observer);
            }
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::sync::Once;

    use jni_sys::{JNI_OK, JNIEnv, JavaVM, jobject};
    use sdl3_sys::system::{SDL_GetAndroidActivity, SDL_GetAndroidJNIEnv};

    static CONTEXT: Once = Once::new();

    pub(super) fn prepare() -> Result<(), String> {
        let mut result = Ok(());
        CONTEXT.call_once(|| result = set_context());
        result
    }

    /// Gives `ndk-context` the app's VM and activity, as a global reference it keeps for the life
    /// of the process.
    fn set_context() -> Result<(), String> {
        // SAFETY: called on SDL's thread, where SDL's JNI environment is valid. The activity is a
        // new local reference, released here; the function table is the JVM's, and the calls below
        // are made as JNI documents them. `initialize_android_context` asserts it runs once, which
        // the `Once` around this holds.
        unsafe {
            let env = SDL_GetAndroidJNIEnv().cast::<JNIEnv>();
            if env.is_null() {
                return Err(format!("SDL has no JNI environment: {}", crate::sdl::last_error()));
            }
            let activity: jobject = SDL_GetAndroidActivity().cast();
            if activity.is_null() {
                return Err(format!("SDL has no activity: {}", crate::sdl::last_error()));
            }
            let table = *env;
            let mut vm: *mut JavaVM = std::ptr::null_mut();
            let got_vm = ((*table).v1_1.GetJavaVM)(env, &raw mut vm);
            let global = ((*table).v1_1.NewGlobalRef)(env, activity);
            ((*table).v1_1.DeleteLocalRef)(env, activity);
            if got_vm != JNI_OK || vm.is_null() || global.is_null() {
                return Err("the JVM gave no VM or reference for the activity".to_owned());
            }
            ndk_context::initialize_android_context(vm.cast(), global.cast());
        }
        Ok(())
    }

    pub(super) fn interrupted() -> bool {
        // Audio focus isn't taken, so nothing interrupts: a call still routes over the stream, and
        // a pause comes as a lifecycle event.
        false
    }

    pub(super) fn activate() -> bool {
        true
    }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod imp {
    pub(super) fn prepare() -> Result<(), String> {
        Ok(())
    }

    pub(super) fn interrupted() -> bool {
        false
    }

    pub(super) fn activate() -> bool {
        true
    }
}
