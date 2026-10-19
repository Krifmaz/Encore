//! Background work: the gate that keeps it out of the way of people using the app ([`Busy`]), the
//! progress of long-running library jobs ([`Jobs`]), the scheduled tasks ([`Scheduler`]), the
//! [`Supervisor`]s of the loops that run as long as the server, and slow answers kept fresh in the
//! background ([`Refreshed`]). The server keeps one [`Busy`], one [`Jobs`] and one [`Scheduler`] in its state.
pub mod busy;
pub mod progress;
pub mod refresh;
pub mod scheduler;
pub mod supervisor;

pub use busy::Busy;
pub use progress::Jobs;
pub use refresh::Refreshed;
pub use scheduler::Scheduler;
pub use supervisor::Supervisor;
