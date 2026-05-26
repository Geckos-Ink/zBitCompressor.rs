// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

pub mod advanced;
pub mod error;
pub mod hierarchical;
pub mod minimizer;
pub mod model;
pub mod pack;
pub mod pack_rules;
pub mod sat;

pub use advanced::{
    evaluate_cover_objective, minimize_advanced, AdvancedMinimization, AdvancedOptions,
    AdvancedReport, MappingObjective, ObjectiveEstimate,
};
pub use error::{ZbitError, ZbitResult};
pub use model::{ZbitModel, ZbitStats, ZBIT_MAX_INPUTS_EXACT};
pub use pack::{
    compress_adaptive_stream_to_file, compress_adaptive_to_file, decompress_file,
    decompress_stream_file, decompress_stream_file_from_key_piece, PackStats, StreamPackOptions,
    StreamPackStats,
};
pub use pack_rules::PackMethod;
