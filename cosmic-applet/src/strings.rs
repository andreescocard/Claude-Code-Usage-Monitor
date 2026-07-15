#[derive(Clone, Copy)]
pub struct Strings {
    pub session_window: &'static str,
    pub weekly_window: &'static str,
    pub now: &'static str,
    pub day_suffix: &'static str,
    pub hour_suffix: &'static str,
    pub minute_suffix: &'static str,
    pub second_suffix: &'static str,
    pub no_credentials: &'static str,
    pub token_expired: &'static str,
}

pub const STRINGS: Strings = Strings {
    session_window: "5h",
    weekly_window: "7d",
    now: "now",
    day_suffix: "d",
    hour_suffix: "h",
    minute_suffix: "m",
    second_suffix: "s",
    no_credentials: "Not signed in",
    token_expired: "Sign-in required",
};
