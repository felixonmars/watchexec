use miette::Diagnostic;
use thiserror::Error;
use watchexec_signals::Signal;

use crate::{
	event::{Event, Priority},
	fs::Watcher,
};

/// Errors which _may_ be recoverable, transient, or only affect a part of the operation, and should
/// be reported to the user and/or acted upon programmatically, but will not outright stop watchexec.
///
/// Some errors that are classified here are spurious and may be ignored; in general you should not
/// use the convenience print handlers for handling these errors beyond prototyping. For example,
/// "waiting on process" errors should not be printed to the user by default:
///
/// ```
/// # use std::convert::Infallible;
/// # use tracing::error;
/// # use watchexec::{config::InitConfig, ErrorHook, error::RuntimeError, handler::SyncFnHandler};
/// # let mut config = InitConfig::default();
/// config.on_error(SyncFnHandler::from(
///     |err: ErrorHook| -> std::result::Result<(), Infallible> {
///         if let RuntimeError::IoError {
///             about: "waiting on process group",
///             ..
///         } = err.error
///         {
///             error!("{}", err.error);
///             return Ok(());
///         }
///
///         // ...
///
///         Ok(())
///     },
/// ));
/// ```
///
/// On the other hand, some errors may not be fatal to this library's understanding, but will be to
/// your application. In those cases, you should "elevate" these errors, which will transform them
/// to [`CriticalError`](super::CriticalError)s:
///
/// ```
/// # use std::convert::Infallible;
/// # use watchexec::{config::InitConfig, ErrorHook, error::{RuntimeError, FsWatcherError}, handler::SyncFnHandler};
/// # let mut config = InitConfig::default();
/// config.on_error(SyncFnHandler::from(
///     |err: ErrorHook| -> std::result::Result<(), Infallible> {
///         if let RuntimeError::FsWatcher {
///             err:
///                 FsWatcherError::Create { .. }
///                 | FsWatcherError::TooManyWatches { .. }
///                 | FsWatcherError::TooManyHandles { .. },
///             ..
///         } = err.error
///         {
///             err.elevate();
///             return Ok(());
///         }
///
///         // ...
///
///         Ok(())
///     },
/// ));
/// ```
#[derive(Debug, Diagnostic, Error)]
#[non_exhaustive]
#[diagnostic(url(docsrs))]
pub enum RuntimeError {
	/// Pseudo-error used to signal a graceful exit.
	#[error("this should never be printed (exit)")]
	#[diagnostic(code(watchexec::runtime::exit))]
	Exit,

	/// For custom runtime errors.
	///
	/// This should be used for errors by external code which are not covered by the other error
	/// types; watchexec-internal errors should never use this.
	#[error("external(runtime): {0}")]
	#[diagnostic(code(watchexec::runtime::external))]
	External(#[from] Box<dyn std::error::Error + Send + Sync>),

	/// Generic I/O error, with some context.
	#[error("io({about}): {err}")]
	#[diagnostic(code(watchexec::runtime::io_error))]
	IoError {
		/// What it was about.
		about: &'static str,

		/// The I/O error which occurred.
		#[source]
		err: std::io::Error,
	},

	/// Events from the filesystem watcher event source.
	#[error("{kind:?} fs watcher error")]
	#[diagnostic(code(watchexec::runtime::fs_watcher))]
	FsWatcher {
		/// The kind of watcher that failed to instantiate.
		kind: Watcher,

		/// The underlying error.
		#[source]
		err: super::FsWatcherError,
	},

	/// Events from the keyboard event source
	#[error("keyboard watcher error")]
	#[diagnostic(code(watchexec::runtime::keyboard_watcher))]
	KeyboardWatcher {
		/// The underlying error.
		#[source]
		err: super::KeyboardWatcherError,
	},

	/// Opaque internal error from a command supervisor.
	#[error("internal: command supervisor: {0}")]
	#[diagnostic(code(watchexec::runtime::internal_supervisor))]
	InternalSupervisor(String),

	/// Error received when an event cannot be sent to the event channel.
	#[error("cannot send event from {ctx}: {err}")]
	#[diagnostic(code(watchexec::runtime::event_channel_send))]
	EventChannelSend {
		/// The context in which this error happened.
		///
		/// This is not stable and its value should not be relied on except for printing the error.
		ctx: &'static str,

		/// The underlying error.
		#[source]
		err: async_priority_channel::SendError<(Event, Priority)>,
	},

	/// Error received when an event cannot be sent to the event channel.
	#[error("cannot send event from {ctx}: {err}")]
	#[diagnostic(code(watchexec::runtime::event_channel_try_send))]
	EventChannelTrySend {
		/// The context in which this error happened.
		///
		/// This is not stable and its value should not be relied on except for printing the error.
		ctx: &'static str,

		/// The underlying error.
		#[source]
		err: async_priority_channel::TrySendError<(Event, Priority)>,
	},

	/// Error received when a [`Handler`][crate::handler::Handler] errors.
	///
	/// The error is completely opaque, having been flattened into a string at the error point.
	#[error("handler error while {ctx}: {err}")]
	#[diagnostic(code(watchexec::runtime::handler))]
	Handler {
		/// The context in which this error happened.
		///
		/// This is not stable and its value should not be relied on except for printing the error.
		ctx: &'static str,

		/// The underlying error, as the Display representation of the original error.
		err: String,
	},

	/// Error received when a [`Handler`][crate::handler::Handler] which has been passed a lock has kept that lock open after the handler has completed.
	#[error("{0} handler returned while holding a lock alive")]
	#[diagnostic(code(watchexec::runtime::handler_lock_held))]
	HandlerLockHeld(&'static str),

	/// Error received when operating on a process.
	#[error("when operating on process: {0}")]
	#[diagnostic(code(watchexec::runtime::process))]
	Process(#[source] std::io::Error),

	/// Error received when a process did not start correctly, or finished before we could even tell.
	#[error("process was dead on arrival")]
	#[diagnostic(code(watchexec::runtime::process_doa))]
	ProcessDeadOnArrival,

	/// Error received when a [`Signal`] is unsupported
	///
	/// This may happen if the signal is not supported on the current platform, or if Watchexec
	/// doesn't support sending the signal.
	#[error("unsupported signal: {0:?}")]
	#[diagnostic(code(watchexec::runtime::unsupported_signal))]
	UnsupportedSignal(Signal),

	/// Error received when there are no commands to run.
	///
	/// This is generally a programmer error and should be caught earlier.
	#[error("no commands to run")]
	#[diagnostic(code(watchexec::runtime::no_commands))]
	NoCommands,

	/// Error received when trying to render a [`Command::Shell`](crate::command::Command) that has no `command`
	///
	/// This is generally a programmer error and should be caught earlier.
	#[error("empty shelled command")]
	#[diagnostic(code(watchexec::runtime::command_shell::empty_command))]
	CommandShellEmptyCommand,

	/// Error received when trying to render a [`Shell::Unix`](crate::command::Shell) with an empty shell
	///
	/// This is generally a programmer error and should be caught earlier.
	#[error("empty shell program")]
	#[diagnostic(code(watchexec::runtime::command_shell::empty_shell))]
	CommandShellEmptyShell,

	/// Error received when clearing the screen.
	#[error("clear screen: {0}")]
	#[diagnostic(code(watchexec::runtime::clearscreen))]
	Clearscreen(#[from] clearscreen::Error),

	/// Error received from the [`ignore-files`](ignore_files) crate.
	#[error("ignore files: {0}")]
	#[diagnostic(code(watchexec::runtime::ignore_files))]
	IgnoreFiles(
		#[diagnostic_source]
		#[from]
		ignore_files::Error,
	),

	/// Error emitted by a [`Filterer`](crate::filter::Filterer).
	#[error("{kind} filterer: {err}")]
	#[diagnostic(code(watchexec::runtime::filterer))]
	Filterer {
		/// The kind of filterer that failed.
		///
		/// This should be set by the filterer itself to a short name for the filterer.
		///
		/// This is not stable and its value should not be relied on except for printing the error.
		kind: &'static str,

		/// The underlying error.
		#[source]
		err: Box<dyn std::error::Error + Send + Sync>,
	},
}
