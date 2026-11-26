use crate::RetryReason;

/// Immutable bilingual UI/TTS copy pair (`ta` / `en`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BilingualCopy {
    pub ta: &'static str,
    pub en: &'static str,
}

/// Out-of-scope refusal displayed in `AiChat` (`Synopsis.pdf` literary boundary).
pub const OOS_CHAT_REFUSAL: BilingualCopy = BilingualCopy {
    ta: "மன்னிக்கவும், திருவள்ளுவர் AI திருக்குறள், அதன் உரைகள் மற்றும் திருவள்ளுவர் வரலாறு தொடர்பான கேள்விகளுக்கு மட்டுமே பதிலளிக்கும்.",
    en: "Thiruvalluvar AI only answers questions about Thirukkural couplets, commentaries, and Thiruvalluvar's biography.",
};

/// Concise (`<= 12` word) spoken out-of-scope TTS prompt for hands-free voice queries.
pub const OOS_VOICE_TTS_REFUSAL: BilingualCopy = BilingualCopy {
    ta: "திருக்குறள் தொடர்பான கேள்விகளுக்கு மட்டுமே என்னால் உதவ முடியும்.",
    en: "I can only help with questions about the Thirukkural.",
};

/// Fallback notice shown when `Parimel` is unavailable (`#395` and `#648`).
pub const PARIMEL_FALLBACK_NOTICE: BilingualCopy = BilingualCopy {
    ta: "இந்தக் குறளுக்குப் பரிமேலழகர் உரை இல்லை; மு.வ. உரை காட்டப்படுகிறது.",
    en: "Parimelazhagar commentary is unavailable for this Kural; showing Mu. Va. instead.",
};

/// Validation error when a voice/input Kural number falls outside `1..=1330`.
pub const VOICE_KURAL_RANGE_ERROR: BilingualCopy = BilingualCopy {
    ta: "குறள் எண் 1 முதல் 1330 வரை மட்டுமே இருக்க வேண்டும்.",
    en: "Kural number must be between 1 and 1330.",
};

/// Validation error when an Adhikaram number falls outside `1..=133`.
pub const VOICE_ADHIKARAM_RANGE_ERROR: BilingualCopy = BilingualCopy {
    ta: "அதிகாரம் எண் 1 முதல் 133 வரை மட்டுமே இருக்க வேண்டும்.",
    en: "Adhikaram number must be between 1 and 133.",
};

/// Status toast shown when replaying the last cached TTS clip.
pub const REPLAY_AUDIO_NOTICE: BilingualCopy = BilingualCopy {
    ta: "மீண்டும் ஒலிக்கிறது...",
    en: "Replaying audio...",
};

/// Status toast shown when replaying the user's captured voice turn.
pub const REPLAY_USER_VOICE_NOTICE: BilingualCopy = BilingualCopy {
    ta: "உங்கள் குரல் மீண்டும் ஒலிக்கிறது...",
    en: "Replaying your voice...",
};

/// Status toast shown when replaying a full Voice-to-Voice exchange.
pub const REPLAY_FULL_EXCHANGE_NOTICE: BilingualCopy = BilingualCopy {
    ta: "முழு உரையாடலும் ஒலிக்கிறது...",
    en: "Replaying conversation...",
};

/// Friendly info notice when no TTS/voice audio has been cached yet.
pub const NOTHING_TO_REPLAY_NOTICE: BilingualCopy = BilingualCopy {
    ta: "மீண்டும் ஒலிக்க எதுவும் இல்லை.",
    en: "Nothing to replay yet.",
};

/// Notice when Auto-Speak Text is turned on.
pub const AUTO_SPEAK_ON_NOTICE: BilingualCopy = BilingualCopy {
    ta: "தானியங்கி பதில் குரல்: இயக்கப்பட்டது.",
    en: "Auto-Speak Text: ON.",
};

/// Notice when Auto-Speak Text is turned off.
pub const AUTO_SPEAK_OFF_NOTICE: BilingualCopy = BilingualCopy {
    ta: "தானியங்கி பதில் குரல்: நிறுத்தப்பட்டது.",
    en: "Auto-Speak Text: OFF.",
};

/// Notice when Voice-to-Voice hands-free loop is enabled.
pub const VOICE_LOOP_ON_NOTICE: BilingualCopy = BilingualCopy {
    ta: "தொடர் குரல் உரையாடல்: இயக்கப்பட்டது.",
    en: "Voice-to-Voice Loop: ON.",
};

/// Notice when Voice-to-Voice hands-free loop is turned off.
pub const VOICE_LOOP_OFF_NOTICE: BilingualCopy = BilingualCopy {
    ta: "தொடர் குரல் உரையாடல்: நிறுத்தப்பட்டது.",
    en: "Voice-to-Voice Loop: OFF.",
};

/// Notice when Voice-to-Voice loop automatically disarms after consecutive silence timeouts.
pub const VOICE_LOOP_SILENCE_STOP_NOTICE: BilingualCopy = BilingualCopy {
    ta: "அமைதி காரணமாக தொடர் குரல் நிறுத்தப்பட்டது.",
    en: "Voice-to-Voice Loop paused after silence.",
};

/// Bilingual confirmation toast when automatic or manual Start-screen voice routing opens Kids mode.
pub const AGE_MODE_OPENED_KIDS: BilingualCopy = BilingualCopy {
    ta: "குழந்தைகள் பகுதி திறக்கப்பட்டது",
    en: "Kids mode opened",
};

/// Bilingual confirmation toast when automatic or manual Start-screen voice routing opens Youth mode.
pub const AGE_MODE_OPENED_YOUTH: BilingualCopy = BilingualCopy {
    ta: "இளைஞர் பகுதி திறக்கப்பட்டது",
    en: "Youth mode opened",
};

/// Bilingual confirmation toast when automatic or manual Start-screen voice routing opens Adult mode.
pub const AGE_MODE_OPENED_ADULT: BilingualCopy = BilingualCopy {
    ta: "பெரியவர் பகுதி திறக்கப்பட்டது",
    en: "Adult mode opened",
};

/// Return locked bilingual retry copy for Voice Age Detection quality rejections.
pub const fn retry_copy(reason: RetryReason) -> BilingualCopy {
    match reason {
        RetryReason::TooShort => BilingualCopy {
            ta: "குரல் பதிவு மிகக் குறுகியது; குறைந்தது 1.2 வினாடிகள் பேசவும்.",
            en: "Speech sample too short; please speak for at least 1.2 seconds.",
        },
        RetryReason::TooQuiet => BilingualCopy {
            ta: "குரல் மிக மெதுவாக உள்ளது; மைக்கிற்கு அருகில் பேசவும்.",
            en: "Audio level too low; please speak closer to the microphone.",
        },
        RetryReason::TooNoisy => BilingualCopy {
            ta: "பின்னணி இரைச்சல் அதிகம்; சற்று அமைதியான சூழலில் மீண்டும் முயலவும்.",
            en: "Background noise is too high; please retry or choose an age card.",
        },
        RetryReason::Clipped => BilingualCopy {
            ta: "ஒலி அளவு அதிகமாகிவிட்டது; மைக்கிலிருந்து சற்று தள்ளிப் பேசவும்.",
            en: "Audio clipped; please step slightly back from the microphone.",
        },
        RetryReason::HardwareUnavailable => BilingualCopy {
            ta: "ஒலிவாங்கி கிடைக்கவில்லை; கீழே உள்ள வயது அட்டையைத் தேர்ந்தெடுக்கவும்.",
            en: "Microphone unavailable; please select an age card below.",
        },
    }
}

/// Format bilingual confirmation toast strings for recognized voice commands.
pub fn format_voice_toast(transcript: &str) -> (String, String) {
    (
        format!("கேட்டது: {transcript}"),
        format!("Heard: {transcript}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::is_nfc;

    #[test]
    fn all_locked_copy_constants_are_non_empty_and_nfc() {
        for copy in [
            OOS_CHAT_REFUSAL,
            OOS_VOICE_TTS_REFUSAL,
            PARIMEL_FALLBACK_NOTICE,
            VOICE_KURAL_RANGE_ERROR,
            VOICE_ADHIKARAM_RANGE_ERROR,
            REPLAY_AUDIO_NOTICE,
            REPLAY_USER_VOICE_NOTICE,
            REPLAY_FULL_EXCHANGE_NOTICE,
            NOTHING_TO_REPLAY_NOTICE,
            AUTO_SPEAK_ON_NOTICE,
            AUTO_SPEAK_OFF_NOTICE,
            VOICE_LOOP_ON_NOTICE,
            VOICE_LOOP_OFF_NOTICE,
            VOICE_LOOP_SILENCE_STOP_NOTICE,
            AGE_MODE_OPENED_KIDS,
            AGE_MODE_OPENED_YOUTH,
            AGE_MODE_OPENED_ADULT,
            retry_copy(RetryReason::TooShort),
            retry_copy(RetryReason::TooQuiet),
            retry_copy(RetryReason::TooNoisy),
            retry_copy(RetryReason::Clipped),
            retry_copy(RetryReason::HardwareUnavailable),
        ] {
            assert!(!copy.ta.is_empty());
            assert!(!copy.en.is_empty());
            assert!(is_nfc(copy.ta));
            assert!(is_nfc(copy.en));
        }
    }
}

