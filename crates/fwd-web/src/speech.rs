//! Reading a message aloud, so a person can hear a question while their hands stay on the reply
//! box (the inbox's double Shift). It goes through the browser's speech synthesis (the Web Speech
//! API). The voice is a setting of this browser, kept in localStorage: until the person picks
//! one, the first of "Ava (Premium)" and "Samantha" the browser offers speaks, else the browser's
//! default voice. Browsers load their voices after the page starts and announce them with
//! `voiceschanged`, so the list is a signal the header's picker reads.

use leptos::prelude::*;
use std::sync::OnceLock;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{SpeechSynthesis, SpeechSynthesisUtterance, SpeechSynthesisVoice};

/// The localStorage key holding the chosen voice's name; an empty value chooses the browser's
/// default voice, and no value at all leaves the choice to `PREFERRED`.
const SETTING: &str = "fwd.web.voice";

/// The voices that speak until the person picks one, the first this browser has winning.
const PREFERRED: [&str; 2] = ["Ava (Premium)", "Samantha"];

static VOICES: OnceLock<ArcRwSignal<Vec<String>>> = OnceLock::new();
static CHOSEN: OnceLock<ArcRwSignal<Option<String>>> = OnceLock::new();

fn synth() -> Option<SpeechSynthesis> {
    web_sys::window()?.speech_synthesis().ok()
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The names of the English voices this browser offers, sorted: empty until it has loaded them,
/// and for good in a browser without speech synthesis.
pub fn voices() -> ArcRwSignal<Vec<String>> {
    VOICES
        .get_or_init(|| {
            let signal = ArcRwSignal::new(english_voices());
            if let Some(synth) = synth() {
                let target = signal.clone();
                let on_change = Closure::<dyn FnMut()>::new(move || target.set(english_voices()));
                synth.set_onvoiceschanged(Some(on_change.as_ref().unchecked_ref()));
                on_change.forget();
            }
            signal
        })
        .clone()
}

fn all_voices(synth: &SpeechSynthesis) -> Vec<SpeechSynthesisVoice> {
    synth.get_voices().iter().filter_map(|v| v.dyn_into::<SpeechSynthesisVoice>().ok()).collect()
}

fn english_voices() -> Vec<String> {
    let Some(synth) = synth() else { return Vec::new() };
    let mut names: Vec<String> = all_voices(&synth)
        .into_iter()
        .filter(|v| v.lang().to_ascii_lowercase().starts_with("en"))
        .map(|v| v.name())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// What the person picked in this browser: `None` when they never picked, `Some("")` for the
/// browser's default voice, else a voice's name.
pub fn chosen() -> ArcRwSignal<Option<String>> {
    CHOSEN.get_or_init(|| ArcRwSignal::new(local_storage().and_then(|s| s.get_item(SETTING).ok().flatten()))).clone()
}

/// Records the person's pick (a voice's name, or `""` for the browser's default voice).
pub fn choose(name: String) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(SETTING, &name);
    }
    chosen().set(Some(name));
}

/// The voice that speaks, among `voices`: the one picked when this browser has it, else the
/// first of `PREFERRED` it has; `None` means the browser's default voice.
pub fn effective(voices: &[String], chosen: Option<&str>) -> Option<String> {
    let offered = |name: &str| voices.iter().any(|v| v == name);
    match chosen {
        Some("") => None,
        Some(name) if offered(name) => Some(name.to_string()),
        _ => PREFERRED.iter().find(|name| offered(name)).map(|name| name.to_string()),
    }
}

/// Speaks `text` in the effective voice, cutting off whatever was being spoken.
pub fn speak(text: &str) {
    let Some(synth) = synth() else { return };
    // Only when something is being spoken: browsers have been reported to drop an utterance
    // queued straight after a `cancel()`.
    if synth.speaking() || synth.pending() {
        synth.cancel();
    }
    let Ok(utterance) = SpeechSynthesisUtterance::new_with_text(text) else { return };
    let name = effective(&voices().get_untracked(), chosen().get_untracked().as_deref());
    if let Some(voice) = name.and_then(|name| all_voices(&synth).into_iter().find(|v| v.name() == name)) {
        utterance.set_voice(Some(&voice));
    }
    synth.speak(&utterance);
}

/// True while something is being spoken or waits to be.
pub fn speaking() -> bool {
    synth().is_some_and(|s| s.speaking() || s.pending())
}

pub fn stop() {
    if let Some(synth) = synth() {
        synth.cancel();
    }
}
