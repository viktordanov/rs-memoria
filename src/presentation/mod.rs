//! Argument parsing and output rendering. Handlers stay thin: they translate
//! arguments into use-case calls and results into text or JSON.

pub mod cli;
pub mod human;
pub mod json;
pub mod text;

pub mod review;
