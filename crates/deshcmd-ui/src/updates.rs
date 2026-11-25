use std::sync::mpsc::{self, TryRecvError};

use eframe::egui;

use crate::{Action, App};

pub struct DesktopUpdate {
    pub version: String,
    pub install: Box<dyn FnOnce() -> Result<(), String> + Send>,
}

pub type UpdateCheck = mpsc::Receiver<Result<Option<DesktopUpdate>, String>>;

pub(crate) enum UpdateDialog {
    Checking(UpdateCheck),
    Ready(DesktopUpdate),
    Message(String),
}

impl App {
    pub fn show_update_error(&mut self, message: String) {
        self.update = Some(UpdateDialog::Message(message));
    }

    pub(crate) fn check_for_updates(&mut self, ctx: &egui::Context) {
        if self.update.is_none() {
            self.update = Some(match self.desktop.check_for_update(ctx.clone()) {
                Ok(check) => UpdateDialog::Checking(check),
                Err(error) => UpdateDialog::Message(error),
            });
        }
    }

    pub(crate) fn poll_update(&mut self) {
        if let Some(UpdateDialog::Checking(check)) = &self.update {
            self.update = Some(match check.try_recv() {
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    UpdateDialog::Message("The update check ended unexpectedly.".into())
                }
                Ok(Err(error)) => UpdateDialog::Message(error),
                Ok(Ok(None)) => UpdateDialog::Message("DeshCmd is up to date.".into()),
                Ok(Ok(Some(update))) => UpdateDialog::Ready(update),
            });
        }
    }

    pub(crate) fn update_dialog(&mut self, ctx: &egui::Context) {
        let Some(update) = &self.update else { return };
        let busy = self.engine.jobs.iter().any(|job| !job.state.finished());
        let response = egui::Modal::new(egui::Id::unique("deshcmd-update")).show(ctx, |ui| {
            ui.set_max_width(440.);
            ui.heading("DeshCmd Updates");
            match update {
                UpdateDialog::Checking(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking for and downloading updates…");
                    });
                    (false, false)
                }
                UpdateDialog::Ready(update) => {
                    ui.label(format!("DeshCmd {} is ready to install.", update.version));
                    if busy {
                        ui.label("Finish or cancel pending file operations to restart DeshCmd.");
                    }
                    ui.horizontal(|ui| {
                        let close = ui.button("Later").clicked();
                        let install = ui
                            .add_enabled(!busy, egui::Button::new("Install and Restart"))
                            .clicked();
                        (close, install)
                    })
                    .inner
                }
                UpdateDialog::Message(message) => {
                    ui.label(message);
                    (ui.button("Close").clicked(), false)
                }
            }
        });
        let (close, install) = response.inner;
        if install && let Some(UpdateDialog::Ready(update)) = self.update.take() {
            match (update.install)() {
                Ok(()) => self.apply(Action::Close, ctx),
                Err(error) => self.show_update_error(error),
            }
        } else if (close || response.should_close())
            && !matches!(self.update, Some(UpdateDialog::Checking(_)))
        {
            self.update = None;
        }
    }
}
