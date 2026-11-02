//! The OS's side of playing sound (M2 E1, spec §5.15): what an app does once, before its first
//! audio stream, and what the OS says afterwards.
//!
//! - **iOS:** the audio session's category and activation, and its interruptions (calls, Siri,
//!   alarms) as a flag the app polls. Without a category an app takes the default, which stops the
//!   player's music, and a media services reset takes it back to that, so a reset sets the session up
//!   anew. When the OS refuses to activate the session after an interruption, a reset or a trip to the
//!   background, it's asked again every couple of seconds for as long as the app goes on polling, so
//!   the sound isn't left silent until the next trip.
//! - **Android:** cpal's AAudio host reads the app's `JavaVM` and activity through `ndk-context`
//!   when it opens a stream, and panics if nobody set them. `cargo-apk`'s glue would; SDL's entry
//!   doesn't, so this does, from SDL's own JNI handles.
//! - **Desktop:** nothing.
//!
//! Only apps that play sound come here, so an app that plays none has no session and takes no
//! audio focus.

/// The session and context for this OS, as often as asked. On the main thread (SDL's thread on
/// Android), where SDL's JNI environment and UIKit's calls are valid.
pub(crate) fn prepare() -> Result<(), String> {
    imp::prepare()
}

/// Whether the OS has taken audio away, as far as the session says. Once a frame, it's also what asks
/// again for a session the OS refused.
pub(crate) fn interrupted() -> bool {
    imp::interrupted()
}

/// Asks for audio again, coming back to the foreground. Returns whether the OS gave it.
pub(crate) fn activate() -> bool {
    imp::activate()
}

/// A try to make again in a while, counted in polls: no clock, and no polls while the app's away.
#[cfg(any(target_os = "ios", test))]
struct RetryIn(std::sync::atomic::AtomicU32);

#[cfg(any(target_os = "ios", test))]
impl RetryIn {
    const fn new() -> Self {
        Self(std::sync::atomic::AtomicU32::new(0))
    }

    /// Makes the next try due on the `polls`th poll from now.
    fn arm(&self, polls: u32) {
        self.0.store(polls, std::sync::atomic::Ordering::Relaxed);
    }

    fn cancel(&self) {
        self.arm(0);
    }

    /// One poll: whether this is the one a try is due on.
    fn poll(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        self.0.try_update(Relaxed, Relaxed, |left| left.checked_sub(1)) == Ok(1)
    }
}

/// Work that must succeed exactly once: what fails runs again when next asked, so its error is never
/// mistaken for a success, and what worked never runs again.
#[cfg(any(target_os = "android", test))]
struct UntilDone(std::sync::Mutex<bool>);

#[cfg(any(target_os = "android", test))]
impl UntilDone {
    const fn new() -> Self {
        Self(std::sync::Mutex::new(false))
    }

    fn run(&self, work: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let mut done = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*done {
            work()?;
            *done = true;
        }
        Ok(())
    }
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
        AVAudioSessionMediaServicesWereResetNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSNumber};

    use super::RetryIn;
    use crate::LOG_TARGET;

    /// The hardware buffer to ask for, in seconds: a hit should be heard within a frame or two, and
    /// iOS's default is about 23 ms. It is only a request, and costs a little more battery.
    const PREFERRED_IO_BUFFER_SECONDS: f64 = 0.010;

    /// How many polls of [`interrupted`] until a refused session is asked again: a couple of seconds at
    /// Mobile-Low's 30 fps. Asking can hold a frame for a moment, and it comes to this only while the OS
    /// is saying no.
    const RETRY_POLLS: u32 = 60;

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);
    static RETRY: RetryIn = RetryIn::new();
    static OBSERVE: Once = Once::new();

    pub(super) fn prepare() -> Result<(), String> {
        set_up_session()?;
        OBSERVE.call_once(observe_session);
        Ok(())
    }

    /// The session as this app plays on it. Only an activation, unless the category's gone (it's the
    /// first time, or media services were reset): setting one on a stream that's playing can reach cpal
    /// as a route change, which opens another stream.
    fn set_up_session() -> Result<(), String> {
        // SAFETY: the shared session may be used from any thread; `Ambient` is a valid category,
        // which lets the player's music play on under the game and follows the silent switch.
        unsafe {
            let session = AVAudioSession::sharedInstance();
            let category = AVAudioSessionCategoryAmbient.ok_or("iOS has no ambient audio category")?;
            if *session.category() != *category {
                session
                    .setCategory_error(category)
                    .map_err(|error| format!("setting the category: {error}"))?;
                if let Err(error) = session.setPreferredIOBufferDuration_error(PREFERRED_IO_BUFFER_SECONDS) {
                    tracing::warn!(target: LOG_TARGET, %error, "the audio session refused a smaller buffer");
                }
            }
            session.setActive_error(true).map_err(|error| format!("activating: {error}"))?;
        }
        Ok(())
    }

    pub(super) fn interrupted() -> bool {
        if RETRY.poll() {
            activate();
        }
        INTERRUPTED.load(Ordering::Relaxed)
    }

    pub(super) fn activate() -> bool {
        match set_up_session() {
            Ok(()) => {
                INTERRUPTED.store(false, Ordering::Relaxed);
                RETRY.cancel();
                true
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "the audio session stays inactive");
                RETRY.arm(RETRY_POLLS);
                false
            }
        }
    }

    /// Follows the session's notifications, for the rest of the process.
    ///
    /// **Interruptions** set [`INTERRUPTED`]. cpal has its own observer, which stops and restarts its
    /// audio unit, but only when the OS suggests a restart. An interruption's end without that suggestion
    /// is an app's to answer, so the flag clears either way, once the session reactivates, however many
    /// tries that takes; the app asks the mixer to go on.
    ///
    /// **A media services reset** leaves the session as if nothing had set it up, and cpal's observer
    /// rebuilds the stream on it through `sapphire_audio`. This one puts the category back. Whichever runs
    /// first, the stream ends up on it: one opened before the category hears of the change as a route
    /// change, and opens again.
    fn observe_session() {
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
                RETRY.cancel();
            } else {
                activate();
            }
        });
        let reset = RcBlock::new(|_: NonNull<NSNotification>| {
            tracing::warn!(target: LOG_TARGET, "media services were reset; setting the audio session up again");
            activate();
        });
        // SAFETY: constants AVFAudio exports. The center copies the blocks; the observer tokens are
        // kept for the life of the process, so they are never removed.
        unsafe {
            let center = NSNotificationCenter::defaultCenter();
            if let Some(name) = AVAudioSessionInterruptionNotification {
                std::mem::forget(center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    None,
                    &block,
                ));
            }
            if let Some(name) = AVAudioSessionMediaServicesWereResetNotification {
                std::mem::forget(center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    None,
                    &reset,
                ));
            }
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use jni_sys::{JNI_OK, JNIEnv, JavaVM, jobject};
    use sdl3_sys::system::{SDL_GetAndroidActivity, SDL_GetAndroidJNIEnv};

    use super::UntilDone;

    static CONTEXT: UntilDone = UntilDone::new();

    pub(super) fn prepare() -> Result<(), String> {
        CONTEXT.run(set_context)
    }

    /// Gives `ndk-context` the app's VM and activity, as a global reference it keeps for the life
    /// of the process.
    fn set_context() -> Result<(), String> {
        // SAFETY: called on SDL's thread, where SDL's JNI environment is valid. The activity is a
        // new local reference, released here; the function table is the JVM's, and the calls below
        // are made as JNI documents them. `initialize_android_context` asserts it runs once, which
        // [`CONTEXT`] holds: it's the last thing here, so nothing that failed has called it.
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

#[cfg(test)]
mod tests {
    use super::{RetryIn, UntilDone};

    #[test]
    fn a_retry_comes_due_once_on_its_poll_and_a_cancel_or_a_new_arm_replaces_it() {
        let retry = RetryIn::new();
        assert!(!retry.poll(), "nothing's armed");
        retry.arm(3);
        assert_eq!([retry.poll(), retry.poll(), retry.poll(), retry.poll()], [false, false, true, false]);
        retry.arm(2);
        assert!(!retry.poll());
        retry.cancel();
        assert!(!retry.poll() && !retry.poll(), "canceled");
        retry.arm(2);
        assert!(!retry.poll());
        retry.arm(2);
        assert_eq!([retry.poll(), retry.poll()], [false, true], "armed again from now");
    }

    /// Android's context: a failure must stay one, since cpal panics opening a stream without it, and
    /// `ndk-context` asserts it's given once.
    #[test]
    fn work_that_failed_runs_again_and_says_so_and_work_that_worked_doesnt() {
        let work = UntilDone::new();
        let mut runs = 0;
        assert!(
            work.run(|| {
                runs += 1;
                Err("no activity".to_owned())
            })
            .is_err()
        );
        assert_eq!(
            work.run(|| {
                runs += 1;
                Err("still none".to_owned())
            }),
            Err("still none".to_owned())
        );
        assert_eq!(
            work.run(|| {
                runs += 1;
                Ok(())
            }),
            Ok(())
        );
        assert_eq!(
            work.run(|| {
                runs += 1;
                Err("never runs".to_owned())
            }),
            Ok(())
        );
        assert_eq!(runs, 3);
    }
}
