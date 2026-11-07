//! 마비노기 MML 변환기 (Rust판): MIDI / MusicXML / 악보 PDF / MML -> PC판 마비노기 MML.

pub mod ai;
pub mod arrange;
pub mod bundle;
pub mod cli;
pub mod config;
pub mod convert;
pub mod encoder;
pub mod midi_io;
pub mod mml_parser;
pub mod mml_split;
pub mod mobile_io;
pub mod model;
pub mod musicxml_io;
pub mod omr;
pub mod player;
pub mod progress;
pub mod pyfmt;
pub mod ranks;
pub mod updater;
pub mod gui;
pub mod sheet;
pub mod theme;
pub mod zipread;

pub const VERSION: &str = "2.4.6";
pub const AUTHOR: &str = "LT 남븝";
