//! Local evaluation and definition refresh lifecycle.

pub(crate) mod coercion;
mod evaluator;
mod murmur;

pub(crate) use evaluator::evaluate_flag;
