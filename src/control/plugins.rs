//! Plugin & marketplace manager.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke, TextEdit, Ui};

use crate::agent::backend::{MarketInfo, MarketPlugin, PluginInfo};
use crate::kit::{self, icon};
use crate::theme::{Theme, mono, semibold, ui as ui_font};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Installed,
    Marketplaces,
}

pub enum PluginAction {
    Refresh,
    Install(String),
    Uninstall(String),
    SetEnabled(String, bool),
    Update(String),
    UpdateAll,
    MarketAdd(String),
    MarketRemove(String),
    Browse(String),
}

#[derive(Default)]
pub struct PluginPanel {
    pub installed: Option<Result<Vec<PluginInfo>, String>>,
    pub markets: Option<Result<Vec<MarketInfo>, String>>,
    pub browse: HashMap<String, Result<Vec<MarketPlugin>, String>>,
    tab: Tab,
    install_source: String,
    market_source: String,
    open_markets: HashSet<String>,
    filter: String,
}

impl PluginPanel {
    pub fn show(&mut self, ui: &mut Ui, theme: &Theme) -> Vec<PluginAction> {
        let mut out = Vec::new();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Plugins").font(semibold(15.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if kit::icon_button(ui, icon::REFRESH, "Refresh", theme).clicked() {
                    out.push(PluginAction::Refresh);
                }
            });
        });
        ui.label(RichText::new("Plugins bundle skills, agents, hooks and MCP servers for Copilot.").font(ui_font(12.0)).color(theme.text_dim));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.selectable_label(self.tab == Tab::Installed, "Installed").clicked() {
                self.tab = Tab::Installed;
            }
            if ui.selectable_label(self.tab == Tab::Marketplaces, "Marketplaces").clicked() {
                self.tab = Tab::Marketplaces;
            }
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().id_salt("plugins-scroll").auto_shrink([false; 2]).show(ui, |ui| match self.tab {
            Tab::Installed => self.installed_ui(ui, theme, &mut out),
            Tab::Marketplaces => self.markets_ui(ui, theme, &mut out),
        });
        out
    }

    fn installed_ui(&mut self, ui: &mut Ui, theme: &Theme, out: &mut Vec<PluginAction>) {
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut self.install_source).hint_text("plugin@marketplace or owner/repo").desired_width(ui.available_width() - 70.0).font(mono(12.0)));
            if kit::primary_button(ui, "Install", theme).clicked() && !self.install_source.trim().is_empty() {
                out.push(PluginAction::Install(self.install_source.trim().to_owned()));
                self.install_source.clear();
            }
        });
        ui.add_space(6.0);
        match &self.installed {
            None => {
                ui.spinner();
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(theme.err));
            }
            Some(Ok(list)) => {
                if list.is_empty() {
                    ui.label(RichText::new("No plugins installed. Browse a marketplace to find some.").color(theme.text_dim));
                } else {
                    ui.horizontal(|ui| {
                        kit::section(ui, &format!("{} installed", list.len()), theme);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if kit::mini_button(ui, "Update all", theme).clicked() {
                                out.push(PluginAction::UpdateAll);
                            }
                        });
                    });
                }
                for p in list.clone() {
                    Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::PUZZLE).font(crate::theme::icon_font(12.0)).color(if p.enabled { theme.accent } else { theme.text_faint }));
                            ui.label(RichText::new(&p.name).font(semibold(13.0)));
                            if let Some(v) = &p.version {
                                ui.label(RichText::new(v).font(ui_font(11.5)).color(theme.text_faint));
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let mut on = p.enabled;
                                if kit::toggle_named(ui, &mut on, theme, &format!("Enable {}", p.name)).changed() {
                                    out.push(PluginAction::SetEnabled(p.name.clone(), on));
                                }
                            });
                        });
                        if let Some(d) = &p.description {
                            ui.add(egui::Label::new(RichText::new(d).font(ui_font(12.0)).color(theme.text_dim)).wrap());
                        }
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("from {}", p.marketplace)).font(ui_font(11.5)).color(theme.text_faint));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if kit::mini_button(ui, "Uninstall", theme).clicked() {
                                    out.push(PluginAction::Uninstall(p.name.clone()));
                                }
                                if kit::mini_button(ui, "Update", theme).clicked() {
                                    out.push(PluginAction::Update(p.name.clone()));
                                }
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
            }
        }
    }

    fn markets_ui(&mut self, ui: &mut Ui, theme: &Theme, out: &mut Vec<PluginAction>) {
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut self.market_source).hint_text("owner/repo or URL").desired_width(ui.available_width() - 60.0).font(mono(12.0)));
            if kit::primary_button(ui, "Add", theme).clicked() && !self.market_source.trim().is_empty() {
                out.push(PluginAction::MarketAdd(self.market_source.trim().to_owned()));
                self.market_source.clear();
            }
        });
        ui.add(TextEdit::singleline(&mut self.filter).hint_text("Filter plugins…").desired_width(f32::INFINITY));
        ui.add_space(6.0);
        let installed: HashSet<String> = match &self.installed {
            Some(Ok(l)) => l.iter().map(|p| p.name.clone()).collect(),
            _ => HashSet::new(),
        };
        match &self.markets {
            None => {
                ui.spinner();
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(theme.err));
            }
            Some(Ok(list)) => {
                for m in list.clone() {
                    let open = self.open_markets.contains(&m.name);
                    Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&m.name).font(semibold(13.0)));
                            if m.is_default {
                                ui.label(RichText::new("default").font(ui_font(11.0)).color(theme.accent));
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if !m.is_default && kit::mini_button(ui, "Remove", theme).clicked() {
                                    out.push(PluginAction::MarketRemove(m.name.clone()));
                                }
                                if kit::mini_button(ui, if open { "Hide" } else { "Browse" }, theme).clicked() {
                                    if open {
                                        self.open_markets.remove(&m.name);
                                    } else {
                                        self.open_markets.insert(m.name.clone());
                                        out.push(PluginAction::Browse(m.name.clone()));
                                    }
                                }
                            });
                        });
                        ui.label(RichText::new(&m.source).font(mono(11.0)).color(theme.text_faint));
                        if open {
                            match self.browse.get(&m.name) {
                                None => {
                                    ui.spinner();
                                }
                                Some(Err(e)) => {
                                    ui.label(RichText::new(e).color(theme.err));
                                }
                                Some(Ok(plugins)) => {
                                    let f = self.filter.to_lowercase();
                                    for p in plugins.iter().filter(|p| f.is_empty() || p.name.to_lowercase().contains(&f) || p.description.as_deref().unwrap_or("").to_lowercase().contains(&f)) {
                                        ui.separator();
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(&p.name).font(semibold(12.5)));
                                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                                if installed.contains(&p.name) {
                                                    ui.label(RichText::new("installed").font(ui_font(11.0)).color(theme.ok));
                                                } else if kit::mini_button(ui, "Install", theme).clicked() {
                                                    out.push(PluginAction::Install(format!("{}@{}", p.name, m.name)));
                                                }
                                            });
                                        });
                                        if let Some(d) = &p.description {
                                            ui.add(egui::Label::new(RichText::new(d).font(ui_font(11.5)).color(theme.text_dim)).wrap());
                                        }
                                    }
                                }
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
            }
        }
    }
}
