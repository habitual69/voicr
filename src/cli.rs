use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "voicr",
    about = "Headless speech-to-text CLI and daemon",
    version,
    long_about = "voicr is a headless speech-to-text tool.\n\nRun as a background daemon, do one-shot transcriptions,\nmanage models, and configure settings from the terminal."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose debug logging
    #[arg(short, long, global = true)]
    pub debug: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run as a background daemon, listening on a Unix socket
    Daemon {
        /// Socket path (default: /tmp/voicr.sock)
        #[arg(short, long)]
        socket: Option<String>,
    },

    /// Record audio and transcribe (one-shot)
    Transcribe {
        /// Read audio from a WAV file instead of the microphone
        #[arg(short, long)]
        file: Option<String>,

        /// Output file path (default: stdout)
        #[arg(short, long)]
        output: Option<String>,

        /// Maximum recording duration in seconds (0 = unlimited, Ctrl+C to stop)
        #[arg(short = 'd', long, default_value_t = 0)]
        duration: u64,

        /// Disable VAD (record raw audio without silence filtering)
        #[arg(long)]
        no_vad: bool,

        /// Wait for silence before auto-stopping (requires VAD)
        #[arg(long)]
        auto_stop: bool,
    },

    /// Send a command to a running daemon
    Send {
        /// Command to send: start, stop, toggle, cancel, status, shutdown
        command: String,

        /// Socket path (default: /tmp/voicr.sock)
        #[arg(short, long)]
        socket: Option<String>,

        /// Wait for and print the daemon's response
        #[arg(short, long)]
        wait: bool,
    },

    /// Model management
    #[command(subcommand)]
    Model(ModelCommands),

    /// Configuration management
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Transcription history
    #[command(subcommand)]
    History(HistoryCommands),

    /// List audio input and output devices
    Devices,

    /// Run in push-to-talk mode with a global hotkey (default: Ctrl+Space)
    Hotkey {
        /// Override the key combo (e.g. "ctrl+space", "alt+shift+r")
        #[arg(short, long)]
        combo: Option<String>,

        /// Print transcription to stdout instead of pasting into the active window
        #[arg(long)]
        no_paste: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ModelCommands {
    /// List all available models
    List,

    /// Download a model
    Download {
        /// Model ID (e.g. parakeet-tdt-0.6b-v3)
        model_id: String,
    },

    /// Delete a downloaded model
    Delete {
        /// Model ID
        model_id: String,
    },

    /// Set the active model for transcription
    Set {
        /// Model ID
        model_id: String,
    },

    /// Show detailed info about a model
    Info {
        /// Model ID
        model_id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Show current configuration
    Show,

    /// Set a configuration value
    Set {
        /// Config key (e.g. model.selected, audio.vad_threshold)
        key: String,
        /// Value to set
        value: String,
    },

    /// Show the path to the config file
    Path,
}

#[derive(Subcommand, Debug)]
pub enum HistoryCommands {
    /// List transcription history
    List {
        /// Maximum number of entries to show
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },

    /// Show a specific history entry
    Get {
        /// Entry ID
        id: i64,
    },

    /// Delete a history entry
    Delete {
        /// Entry ID
        id: i64,
    },

    /// Toggle saved status of an entry
    Save {
        /// Entry ID
        id: i64,
    },

    /// Export history to JSON
    Export {
        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<String>,
    },
}
