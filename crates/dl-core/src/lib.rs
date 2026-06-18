//! `dl-core` — shared foundations for damascus_laundry.
//!
//! Contains the fixed-point value-path math ([`fixed`], [`amount`]) and the injectable
//! nondeterministic dependencies ([`clock`], [`rng`], [`feed`]) that make the engine
//! deterministic-by-construction.
//!
//! **Invariant:** no `f32`/`f64` in the value/balance/PnL path. Floating point is confined
//! to display helpers at the boundary.

pub mod amount;
pub mod clock;
pub mod display;
pub mod feed;
pub mod fixed;
pub mod prob;
pub mod rng;

pub use amount::Amount;
pub use clock::{Clock, MockClock, Slot, SystemClock};
pub use feed::{Feed, FeedEvent, ScriptedFeed};
pub use fixed::MathError;
pub use prob::{bps_to_prob, mul_prob, prob_ge, RngExt, PROB_SCALE_1E18};
pub use rng::{Rng, SeededRng};
