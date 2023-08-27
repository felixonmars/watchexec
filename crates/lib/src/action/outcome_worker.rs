use std::sync::{
	atomic::{AtomicUsize, Ordering},
	Arc,
};

use async_priority_channel as priority;
use clearscreen::ClearScreen;
use futures::{
	future::{select, Either},
	Future,
};
use tokio::{spawn, sync::mpsc, time::sleep};
use tracing::{debug, error, info, trace, warn};
use watchexec_events::{Event, Priority};

use crate::{
	command::{Args, Command, Supervisor, SupervisorId},
	error::RuntimeError,
	Config,
};

use super::{process_holder::ProcessHolder, Outcome};

#[derive(Clone)]
pub struct OutcomeWorker {
	config: Arc<Config>,
	events: Arc<[Event]>,
	command: Command,
	process: ProcessHolder,
	supervisor_id: SupervisorId,
	gen: usize,
	gencheck: Arc<AtomicUsize>,
	errors_c: mpsc::Sender<RuntimeError>,
	events_c: priority::Sender<Event, Priority>,
}

impl OutcomeWorker {
	pub fn newgen() -> Arc<AtomicUsize> {
		Default::default()
	}

	#[allow(clippy::too_many_arguments)]
	pub fn spawn(
		config: Arc<Config>,
		outcome: Outcome,
		events: Arc<[Event]>,
		command: Command,
		process: ProcessHolder,
		supervisor_id: SupervisorId,
		gencheck: Arc<AtomicUsize>,
		errors_c: mpsc::Sender<RuntimeError>,
		events_c: priority::Sender<Event, Priority>,
	) {
		let gen = gencheck.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
		let this = Self {
			config,
			events,
			command,
			process,
			supervisor_id,
			gen,
			gencheck,
			errors_c,
			events_c,
		};

		debug!(?outcome, %gen, "spawning outcome worker");
		spawn(async move {
			let errors_c = this.errors_c.clone();
			match this.apply(outcome.clone()).await {
				Err(err) => {
					if matches!(err, RuntimeError::Exit) {
						info!(%gen, "propagating graceful exit");
					} else {
						error!(?err, %gen, "outcome applier errored");
					}

					if let Err(err) = errors_c.send(err).await {
						error!(?err, %gen, "failed to send an error, something is terribly wrong");
					}
				}
				Ok(_) => {
					debug!(?outcome, %gen, "outcome worker finished");
				}
			}
		});
	}

	async fn check_gen<O>(&self, f: impl Future<Output = O> + Send) -> Option<O> {
		// TODO: use a select and a notifier of some kind so it cancels tasks
		if self.gencheck.load(Ordering::SeqCst) != self.gen {
			warn!(when=%"pre", gen=%self.gen, "outcome worker was cycled, aborting");
			return None;
		}
		let o = f.await;
		if self.gencheck.load(Ordering::SeqCst) != self.gen {
			warn!(when=%"post", gen=%self.gen, "outcome worker was cycled, aborting");
			return None;
		}
		Some(o)
	}

	#[async_recursion::async_recursion]
	async fn apply(&self, outcome: Outcome) -> Result<(), RuntimeError> {
		macro_rules! notry {
			($e:expr) => {
				match self.check_gen($e).await {
					None => return Ok(()),
					Some(o) => o,
				}
			};
		}
		match (notry!(self.process.is_some()), outcome) {
			(_, Outcome::DoNothing) => {}
			(_, Outcome::Exit) => {
				return Err(RuntimeError::Exit);
			}
			(true, Outcome::Stop) => {
				notry!(self.process.kill());
				notry!(self.process.wait())?;
				notry!(self.process.drop_inner());
			}
			(running, Outcome::Destroy) => {
				if running {
					notry!(self.process.kill());
					notry!(self.process.wait())?;
					notry!(self.process.drop_inner());
				}

				todo!("implement destroy")
			}
			(false, o @ (Outcome::Stop | Outcome::Wait | Outcome::Signal(_))) => {
				debug!(outcome=?o, "meaningless without a process, not doing anything");
			}
			(_, Outcome::Start) => {
				trace!("spawning supervisor for command");
				let sup = Supervisor::spawn(Args {
					config: self.config.clone(),
					errors: self.errors_c.clone(),
					events: self.events_c.clone(),
					command: self.command.clone(),
					supervisor_id: self.supervisor_id,
					actioned_events: self.events.clone(),
					pre_spawn_handler: None,
				})?;
				notry!(self.process.replace(sup));
			}
			(_, Outcome::StartHook(handler)) => {
				trace!("spawning supervisor for command");
				let sup = Supervisor::spawn(Args {
					config: self.config.clone(),
					errors: self.errors_c.clone(),
					events: self.events_c.clone(),
					command: self.command.clone(),
					supervisor_id: self.supervisor_id,
					actioned_events: self.events.clone(),
					pre_spawn_handler: Some(handler),
				})?;
				notry!(self.process.replace(sup));
			}

			(true, Outcome::Signal(sig)) => {
				notry!(self.process.signal(sig));
			}

			(true, Outcome::Wait) => {
				notry!(self.process.wait())?;
			}

			(_, Outcome::Sleep(time)) => {
				trace!(?time, "sleeping");
				notry!(sleep(time));
				trace!(?time, "done sleeping");
			}

			(_, Outcome::Clear) => {
				clearscreen::clear()?;
			}

			(_, Outcome::Reset) => {
				for cs in [
					ClearScreen::WindowsCooked,
					ClearScreen::WindowsVt,
					ClearScreen::VtLeaveAlt,
					ClearScreen::VtWellDone,
					ClearScreen::default(),
				] {
					cs.clear()?;
				}
			}

			(_, Outcome::Hook(handler)) => {
				// TODO: payload? thread?
				handler.call(());
			}

			(true, Outcome::IfRunning(then, _)) => {
				notry!(self.apply(*then))?;
			}
			(false, Outcome::IfRunning(_, otherwise)) => {
				notry!(self.apply(*otherwise))?;
			}

			(_, Outcome::Both(one, two)) => {
				if let Err(err) = notry!(self.apply(*one)) {
					debug!(
                        "first outcome failed, sending an error but proceeding to the second anyway"
                    );
					notry!(self.errors_c.send(err)).ok();
				}

				notry!(self.apply(*two))?;
			}

			(_, Outcome::Race(one, two)) => {
				if let Either::Left((Err(err), _)) | Either::Right((Err(err), _)) =
					select(self.apply(*one), self.apply(*two)).await
				{
					return Err(err);
				}
			}
		}

		Ok(())
	}
}
