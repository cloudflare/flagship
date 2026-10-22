//! Local evaluation and definition refresh lifecycle.

mod coercion;
mod evaluator;
mod murmur;

pub(crate) use evaluator::evaluate_flag;
