//! The HTTP service.
//!
//! Two files, split by the question they answer:
//!   - [`runtime`] how the service comes up: state, router, startup order
//!   - [`events`] what it accepts and answers: request and response types
//!     plus the handlers between them
//!
//! `events` depends only on `engine` and `storage`, never on how the process
//! was started.

mod events;
mod runtime;

pub use runtime::start;
