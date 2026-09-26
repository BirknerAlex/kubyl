//! OperatorHub (board 7): the packages of every CatalogSource as a grid, with search,
//! categories, capability levels, provider and catalog filters, a package's details and the
//! install dialog. Its own tab per cluster.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    IntoElement, KeyBinding, Render, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, Window, actions, canvas, div, prelude::*, px, uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, ResourceRef, TabView, ViewKind,
    ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex,
};

use crate::olm::hub::{self, CAPABILITY_LEVELS, Filters, Package};
use crate::service::{Availability, Icon as PackageIcon, Olm, OlmLease};
use crate::widgets;

/// `ViewKind::Custom` of the OperatorHub tab.
pub const VIEW_KIND: &str = "operatorhub";
pub const CONTEXT: &str = "OperatorHub";

const CARD_MIN_WIDTH: f32 = 230.0;
const CARD_HEIGHT: f32 = 122.0;
const GAP: f32 = 12.0;

actions!(
    operatorhub,
    [
        /// Shows or hides the selected package's details.
        PackageDetails,
        /// Installs the selected package.
        InstallPackage,
        /// Lists the catalogs' packages again.
        ReloadCatalogs,
        /// Focuses the search.
        FocusSearch,
        /// Back from the search to the grid.
        BlurSearch,
        /// Next package.
        NextPackage,
        /// Previous package.
        PreviousPackage,
        /// Opens the active cluster's OperatorHub.
        ShowOperatorHub,
    ]
);

#[derive(Default)]
struct PendingPackages(HashMap<ClusterId, String>);

impl Global for PendingPackages {}

fn request(cluster: &ClusterId) -> ViewRequest {
    ViewRequest::for_resource(
        ViewKind::Custom(VIEW_KIND.into()),
        ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), None),
    )
}

/// Opens (or focuses) OperatorHub for `cluster`.
pub fn open(cluster: &ClusterId, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(OpenView(request(cluster))), cx);
}

/// Opens OperatorHub with a package selected.
pub fn open_package(cluster: &ClusterId, package: &str, window: &mut Window, cx: &mut App) {
    cx.default_global::<PendingPackages>()
        .0
        .insert(cluster.clone(), package.to_string());
    open(cluster, window, cx);
    cx.refresh_windows();
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let cluster = request.target.as_ref()?.cluster.clone();
            Some(Box::new(
                cx.new(|cx| OperatorHubView::new(cluster, window, cx)),
            ))
        },
    );
    for (spec, keys) in [
        (
            ActionSpec::new("OperatorHub: Details", PackageDetails).hint("Details"),
            "enter",
        ),
        (
            ActionSpec::new("OperatorHub: Install…", InstallPackage).hint("Install…"),
            "i",
        ),
        (
            ActionSpec::new("OperatorHub: Reload Catalogs", ReloadCatalogs).hint("Reload catalogs"),
            "shift-r",
        ),
    ] {
        ActionRegistry::register(cx, spec.bind(keys, Some(CONTEXT)));
    }
    cx.bind_keys([
        KeyBinding::new("j", NextPackage, Some(CONTEXT)),
        KeyBinding::new("right", NextPackage, Some(CONTEXT)),
        KeyBinding::new("k", PreviousPackage, Some(CONTEXT)),
        KeyBinding::new("left", PreviousPackage, Some(CONTEXT)),
        KeyBinding::new("/", FocusSearch, Some(CONTEXT)),
        KeyBinding::new("escape", BlurSearch, Some("OperatorHubView > Input")),
    ]);
    ActionRegistry::register(
        cx,
        ActionSpec::new("Operators: Browse OperatorHub", ShowOperatorHub),
    );
    cx.on_action(|_: &ShowOperatorHub, cx| {
        let Some(cluster) = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone())
        else {
            return;
        };
        crate::view::with_window(cx, move |window, cx| open(&cluster, window, cx));
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sort {
    Relevance,
    Name,
}

pub struct OperatorHubView {
    cluster: ClusterId,
    search: Entity<InputState>,
    filters: Filters,
    sort: Sort,
    selected: Option<String>,
    details_open: bool,
    description_open: bool,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// The grid's width (measured), for the number of columns.
    width: Rc<Cell<f32>>,
    /// The filtered packages and what they were filtered from.
    shown: Vec<Arc<Package>>,
    shown_from: Option<(Filters, Sort, usize)>,
    _lease: Option<OlmLease>,
    _subscriptions: Vec<Subscription>,
}

impl OperatorHubView {
    pub fn new(cluster: ClusterId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder("Search packages, providers, APIs"));
        let mut subscriptions = vec![cx.subscribe_in(
            &search,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.filters.query = input.read(cx).value().to_string();
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                _ => {}
            },
        )];
        if let Some(olm) = Olm::global(cx) {
            subscriptions.push(cx.observe(&olm, |_, _, cx| cx.notify()));
        }
        let lease = Olm::watch(&cluster, cx);
        Self {
            cluster,
            search,
            filters: Filters::default(),
            sort: Sort::Relevance,
            selected: None,
            details_open: true,
            description_open: false,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            width: Rc::new(Cell::new(900.0)),
            shown: Vec::new(),
            shown_from: None,
            _lease: lease,
            _subscriptions: subscriptions,
        }
    }

    fn columns(&self, window: &Window) -> usize {
        let scale = f32::from(window.rem_size()) / 16.0;
        let width = self.width.get() / scale - 2.0 * 14.0;
        (((width + GAP) / (CARD_MIN_WIDTH + GAP)).floor() as usize).max(1)
    }

    fn packages(&self, cx: &App) -> Option<Arc<Vec<Arc<Package>>>> {
        Olm::global(cx)?
            .read(cx)
            .hub_state(&self.cluster)?
            .packages
            .clone()
    }

    fn installed(&self, cx: &App) -> BTreeSet<(String, String)> {
        Olm::global(cx)
            .and_then(|o| o.read(cx).snapshot(&self.cluster, cx))
            .map(|s| {
                s.subscriptions
                    .iter()
                    .map(|sub| (sub.package.clone(), sub.source.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn is_installed(installed: &BTreeSet<(String, String)>, package: &Package) -> bool {
        installed.contains(&(package.name.clone(), package.catalog.clone()))
    }

    /// Filters and sorts the packages when something changed.
    fn refresh_shown(&mut self, packages: &Arc<Vec<Arc<Package>>>, cx: &App) {
        let key = (
            self.filters.clone(),
            self.sort,
            Arc::as_ptr(packages) as usize,
        );
        if self.shown_from.as_ref() == Some(&key) {
            return;
        }
        let installed = self.installed(cx);
        let mut shown: Vec<Arc<Package>> = packages
            .iter()
            .filter(|p| self.filters.matches(p, Self::is_installed(&installed, p)))
            .cloned()
            .collect();
        if self.sort == Sort::Relevance && !self.filters.query.trim().is_empty() {
            shown.sort_by_key(|p| self.filters.rank(p));
        }
        self.shown = shown;
        self.shown_from = Some(key);
    }

    fn selected_package(&self) -> Option<Arc<Package>> {
        let key = self.selected.as_ref()?;
        self.shown.iter().find(|p| &p.key() == key).cloned()
    }

    fn select_index(&mut self, index: usize, window: &Window, cx: &mut Context<Self>) {
        let Some(package) = self.shown.get(index) else {
            return;
        };
        self.selected = Some(package.key());
        self.description_open = false;
        let columns = self.columns(window);
        self.scroll
            .scroll_to_item(index / columns, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, window: &Window, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|k| self.shown.iter().position(|p| &p.key() == k));
        let next = match current {
            None => 0,
            Some(i) => (i as isize + delta).clamp(0, self.shown.len() as isize - 1) as usize,
        };
        self.select_index(next, window, cx);
    }

    fn read_only(&self, cx: &App) -> bool {
        ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(&self.cluster).read_only)
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let installed = self.installed(cx);
        if let Some(package) = self.selected_package()
            && !Self::is_installed(&installed, &package)
        {
            crate::dialogs::open_install(self.cluster.clone(), package, window, cx);
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(olm) = Olm::global(cx) {
            let cluster = self.cluster.clone();
            olm.update(cx, |olm, cx| {
                olm.hub(&cluster, true, cx);
            });
        }
    }

    fn render_filters(
        &self,
        packages: &Arc<Vec<Arc<Package>>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let categories = hub::categories(packages);
        let title = |text: &'static str| {
            div()
                .px(u(8.0))
                .pb(u(4.0))
                .text_size(u(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_dim)
                .child(text)
        };
        let category_row = |id: SharedString,
                            label: String,
                            count: usize,
                            on: bool,
                            value: Option<String>,
                            cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .h(u(24.0))
                .px(u(8.0))
                .rounded(u(5.0))
                .cursor_pointer()
                .text_size(u(12.5))
                .when(on, |this| this.bg(colors.selection).text_color(colors.text))
                .when(!on, |this| {
                    this.text_color(colors.text_muted)
                        .hover(|s| s.bg(colors.hover))
                })
                .child(div().flex_1().truncate().child(label))
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child(count.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filters.category = value.clone();
                    cx.notify();
                }))
        };
        let mut category_rows = vec![
            category_row(
                "category-all".into(),
                "All".into(),
                packages.len(),
                self.filters.category.is_none(),
                None,
                cx,
            )
            .into_any_element(),
        ];
        for (i, (name, count)) in categories.iter().enumerate() {
            category_rows.push(
                category_row(
                    SharedString::from(format!("category-{i}")),
                    name.clone(),
                    *count,
                    self.filters.category.as_ref() == Some(name),
                    Some(name.clone()),
                    cx,
                )
                .into_any_element(),
            );
        }
        let capability_rows: Vec<AnyElement> = CAPABILITY_LEVELS
            .iter()
            .enumerate()
            .map(|(i, level)| {
                let on = self.filters.capabilities.contains(*level);
                check_row(("capability", i), on, level.to_string(), None, &colors)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.filters.capabilities.remove(*level) {
                            this.filters.capabilities.insert(level.to_string());
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let catalogs: Vec<(String, String, Option<String>)> = Olm::global(cx)
            .and_then(|o| o.read(cx).snapshot(&self.cluster, cx))
            .map(|s| {
                s.catalogs
                    .iter()
                    .map(|c| {
                        (
                            format!("{}/{}", c.namespace, c.name),
                            c.label(),
                            c.state.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let catalog_rows: Vec<AnyElement> = catalogs
            .into_iter()
            .enumerate()
            .map(|(i, (key, label, state))| {
                let on = !self.filters.hidden_catalogs.contains(&key);
                let detail = format!("{key} · {}", state.unwrap_or_else(|| "unknown".into()));
                check_row(("catalog", i), on, label, Some(detail), &colors)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.filters.hidden_catalogs.remove(&key) {
                            this.filters.hidden_catalogs.insert(key.clone());
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let providers = hub::providers(packages);
        let weak = cx.entity().downgrade();
        let current = self.filters.provider.clone();
        let provider_menu = MenuButton::new("hub-provider")
            .outline()
            .compact()
            .child(
                h_flex()
                    .gap(u(6.0))
                    .text_size(u(12.5))
                    .child(
                        div()
                            .max_w(u(150.0))
                            .truncate()
                            .child(current.clone().unwrap_or_else(|| "Any provider".into())),
                    )
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.max_h(px(420.0)).scrollable(true);
                let pick = |value: Option<String>| {
                    let weak = weak.clone();
                    move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                        let value = value.clone();
                        weak.update(cx, |this, cx| {
                            this.filters.provider = value;
                            cx.notify();
                        })
                        .ok();
                    }
                };
                menu = menu.item(
                    PopupMenuItem::new("Any provider")
                        .checked(current.is_none())
                        .on_click(pick(None)),
                );
                for provider in &providers {
                    menu = menu.item(
                        PopupMenuItem::new(provider.clone())
                            .checked(current.as_ref() == Some(provider))
                            .on_click(pick(Some(provider.clone()))),
                    );
                }
                menu
            });
        v_flex()
            .id("hub-filters")
            .flex_none()
            .w(u(220.0))
            .h_full()
            .overflow_y_scroll()
            .px(u(10.0))
            .py(u(12.0))
            .gap(u(14.0))
            .border_r_1()
            .border_color(colors.border_variant)
            .child(v_flex().child(title("CATEGORY")).children(category_rows))
            .child(
                v_flex()
                    .gap(u(6.0))
                    .child(title("CAPABILITY LEVEL"))
                    .child(v_flex().px(u(8.0)).gap(u(6.0)).children(capability_rows)),
            )
            .child(
                v_flex()
                    .gap(u(4.0))
                    .child(title("PROVIDER"))
                    .child(div().px(u(8.0)).child(provider_menu)),
            )
            .child(
                v_flex()
                    .gap(u(6.0))
                    .child(title("CATALOG"))
                    .child(v_flex().px(u(8.0)).gap(u(6.0)).children(catalog_rows)),
            )
            .child(
                v_flex().gap(u(6.0)).child(title("STATE")).child(
                    div().px(u(8.0)).child(
                        check_row(
                            "installed-only",
                            self.filters.installed_only,
                            "Installed only".into(),
                            None,
                            &colors,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.filters.installed_only = !this.filters.installed_only;
                            cx.notify();
                        })),
                    ),
                ),
            )
            .into_any_element()
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.columns(window);
        let installed = self.installed(cx);
        let olm = Olm::global(cx);
        range
            .map(|row| {
                let cards: Vec<AnyElement> = (0..columns)
                    .map(|col| {
                        let index = row * columns + col;
                        let Some(package) = self.shown.get(index).cloned() else {
                            return div().flex_1().min_w_0().into_any_element();
                        };
                        let icon = olm.as_ref().and_then(|olm| {
                            match olm.update(cx, |o, cx| o.icon(&self.cluster, &package, cx)) {
                                PackageIcon::Loaded(image) => Some(image),
                                _ => None,
                            }
                        });
                        let selected = self.selected.as_ref() == Some(&package.key());
                        self.card(
                            index,
                            &package,
                            icon,
                            selected,
                            Self::is_installed(&installed, &package),
                            &colors,
                            cx,
                        )
                    })
                    .collect();
                h_flex()
                    .id(("hub-row", row))
                    .w_full()
                    .h(u(CARD_HEIGHT + GAP))
                    .px(u(14.0))
                    .pt(u(GAP))
                    .gap(u(GAP))
                    .items_start()
                    .children(cards)
                    .into_any_element()
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn card(
        &self,
        index: usize,
        package: &Package,
        icon: Option<Arc<gpui::Image>>,
        selected: bool,
        installed: bool,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = colors.hover;
        v_flex()
            .id(("package", index))
            .flex_1()
            .min_w_0()
            .h(u(CARD_HEIGHT))
            .p(u(12.0))
            .gap(u(8.0))
            .rounded(u(8.0))
            .border_1()
            .border_color(if selected {
                colors.accent
            } else {
                colors.border
            })
            .bg(if selected {
                colors.selection
            } else {
                colors.subheader_background
            })
            .when(!selected, |this| this.hover(move |s| s.bg(hover)))
            .cursor_pointer()
            .child(
                h_flex()
                    .gap(u(10.0))
                    .child(widgets::tile(
                        package.initials(),
                        &package.name,
                        icon,
                        34.0,
                        colors,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(package.display_name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(package.provider.clone()),
                            ),
                    ),
            )
            .child(
                div()
                    .h(u(34.0))
                    .overflow_hidden()
                    .text_size(u(12.0))
                    .line_height(u(17.0))
                    .text_color(colors.text_muted)
                    .child(package.summary.clone()),
            )
            .child(
                h_flex()
                    .gap(u(4.0))
                    .overflow_hidden()
                    .when_some(package.capability.clone(), |this, capability| {
                        this.child(Chip::new(capability))
                    })
                    .when(installed, |this| {
                        this.child(
                            Chip::new("Installed")
                                .icon(IconName::Check)
                                .text_color(colors.green),
                        )
                    })
                    .when(package.deprecated.is_some(), |this| {
                        this.child(Chip::new("Deprecated").text_color(colors.yellow))
                    }),
            )
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.focus.focus(window, cx);
                    this.select_index(index, window, cx);
                    this.details_open = true;
                    if event.click_count() == 2 {
                        this.install(window, cx);
                    }
                }),
            )
            .into_any_element()
    }

    fn render_details(&mut self, package: Arc<Package>, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let icon = Olm::global(cx).and_then(|olm| {
            match olm.update(cx, |o, cx| o.icon(&self.cluster, &package, cx)) {
                PackageIcon::Loaded(image) => Some(image),
                _ => None,
            }
        });
        let installed = Olm::global(cx)
            .and_then(|o| o.read(cx).snapshot(&self.cluster, cx))
            .and_then(|s| s.installed(&package.name, &package.catalog).cloned());
        let read_only = self.read_only(cx);
        let head = package.head().cloned();
        let cluster = self.cluster.clone();
        let install_package = package.clone();
        let mut action = h_flex().gap(u(8.0));
        match &installed {
            Some(op) => {
                let key = op.key.clone();
                let open_cluster = cluster.clone();
                action = action
                    .child(
                        h_flex()
                            .gap(u(5.0))
                            .text_size(u(12.0))
                            .text_color(colors.green)
                            .child(Icon::new(IconName::Check).size(12.0).color(colors.green))
                            .child(format!(
                                "{} installed in {}",
                                op.version().unwrap_or_default(),
                                op.namespace()
                            )),
                    )
                    .child(
                        Button::new("hub-show-installed")
                            .ghost()
                            .label("Show")
                            .on_click(move |_, window, cx| {
                                crate::view::open(
                                    &open_cluster,
                                    crate::view::Pending {
                                        tab: Some(crate::view::SubTab::Installed),
                                        select: Some(key.clone()),
                                    },
                                    window,
                                    cx,
                                )
                            }),
                    );
            }
            None if !read_only => {
                action = action.child(
                    Button::new("hub-install")
                        .primary()
                        .icon(IconName::Download)
                        .label("Install…")
                        .on_click(move |_, window, cx| {
                            crate::dialogs::open_install(
                                cluster.clone(),
                                install_package.clone(),
                                window,
                                cx,
                            )
                        }),
                );
            }
            None => {
                action = action.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("Read-only cluster: installing is off."),
                );
            }
        }
        let mut rows = Vec::new();
        if let Some(head) = &head {
            rows.push((
                "Latest",
                h_flex()
                    .gap(u(6.0))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(head.version.clone().unwrap_or_default()),
                    )
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .child(format!("· {} (default)", head.name)),
                    )
                    .into_any_element(),
            ));
        }
        rows.push((
            "Channels",
            widgets::text(
                package
                    .channels
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" · "),
            ),
        ));
        if let Some(capability) = &package.capability {
            rows.push(("Capability", widgets::text(capability.clone())));
        }
        if let Some(head) = &head {
            let modes: Vec<&str> = head.install_modes.iter().map(|m| m.label()).collect();
            rows.push(("Install modes", widgets::text(modes.join(", "))));
            if let Some(min) = &head.min_kube_version {
                rows.push((
                    "Min Kubernetes",
                    widgets::mono(min.trim_end_matches("-0").to_string()),
                ));
            }
        }
        if let Some(repository) = package
            .repository
            .clone()
            .filter(|r| r.starts_with("https://") || r.starts_with("http://"))
        {
            let url = repository.clone();
            rows.push((
                "Repository",
                widgets::link(
                    "hub-repository",
                    repository.trim_start_matches("https://").to_string(),
                    &colors,
                    move |_, _, cx| widgets::open_url(&url, cx),
                )
                .into_any_element(),
            ));
        }
        if let Some(image) = &package.container_image {
            rows.push(("Image", widgets::mono(image.clone())));
        }
        rows.push((
            "Catalog",
            widgets::text(format!(
                "{} · {}/{}",
                package.catalog_display, package.catalog_namespace, package.catalog
            )),
        ));
        let description = if package.description.is_empty() {
            package.summary.clone()
        } else {
            hub::plain_text(&package.description)
        };
        let long = description.len() > 700;
        let shown_description = if long && !self.description_open {
            let mut cut = 700;
            while !description.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}…", &description[..cut])
        } else {
            description.clone()
        };
        let owned: Vec<String> = head
            .as_ref()
            .map(|c| c.owned.iter().map(|o| o.kind.clone()).collect())
            .unwrap_or_default();
        v_flex()
            .flex_none()
            .w(u(360.0))
            .h_full()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .flex_none()
                    .items_start()
                    .px(u(14.0))
                    .py(u(12.0))
                    .gap(u(10.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(widgets::tile(
                        package.initials(),
                        &package.name,
                        icon,
                        34.0,
                        &colors,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(u(14.0))
                                    .child(package.display_name.clone()),
                            )
                            .child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                                format!("{} · {}", package.provider, package.catalog_display),
                            )),
                    )
                    .child(
                        kubyl_ui::IconButton::new("hub-details-close", IconName::X)
                            .icon_size(13.0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.details_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("hub-details")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(widgets::plain_section(&colors).child(action))
                    .when_some(package.deprecated.clone(), |this, text| {
                        this.child(widgets::plain_section(&colors).child(widgets::note(
                            IconName::TriangleAlert,
                            colors.yellow,
                            text,
                            &colors,
                        )))
                    })
                    .child(widgets::plain_section(&colors).child(widgets::kv(rows, &colors)))
                    .child(
                        widgets::section("Description", &colors)
                            .child(
                                div()
                                    .text_size(u(12.5))
                                    .line_height(u(18.0))
                                    .text_color(colors.text_muted)
                                    .child(shown_description),
                            )
                            .when(long, |this| {
                                this.child(
                                    widgets::link(
                                        "hub-more",
                                        if self.description_open {
                                            "Less"
                                        } else {
                                            "More"
                                        },
                                        &colors,
                                        {
                                            let weak = cx.entity().downgrade();
                                            move |_, _, cx| {
                                                weak.update(cx, |this, cx| {
                                                    this.description_open = !this.description_open;
                                                    cx.notify();
                                                })
                                                .ok();
                                            }
                                        },
                                    )
                                    .text_size(u(12.0)),
                                )
                            }),
                    )
                    .child(
                        widgets::section("Provided APIs", &colors).child(
                            h_flex()
                                .flex_wrap()
                                .gap(u(4.0))
                                .children(owned.into_iter().map(|k| Chip::new(k).mono())),
                        ),
                    ),
            )
            .into_any_element()
    }
}

fn check_row(
    id: impl Into<gpui::ElementId>,
    on: bool,
    label: String,
    detail: Option<String>,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    h_flex()
        .id(id.into())
        .items_start()
        .gap(u(8.0))
        .cursor_pointer()
        .child(
            div()
                .flex_none()
                .mt(u(2.0))
                .size(u(14.0))
                .rounded(u(3.0))
                .flex()
                .items_center()
                .justify_center()
                .map(|this| {
                    if on {
                        this.bg(colors.accent).child(
                            Icon::new(IconName::Check)
                                .size(10.0)
                                .color(colors.on_accent),
                        )
                    } else {
                        this.border_1().border_color(colors.text_faint)
                    }
                }),
        )
        .child(
            v_flex()
                .min_w_0()
                .child(div().text_size(u(12.5)).child(label))
                .when_some(detail, |this, detail| {
                    this.child(
                        div()
                            .truncate()
                            .text_size(u(11.0))
                            .text_color(colors.text_dim)
                            .child(detail),
                    )
                }),
        )
}

impl Focusable for OperatorHubView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for OperatorHubView {
    fn tab_title(&self, cx: &App) -> SharedString {
        let active =
            ActiveContext::global(cx).cluster.as_ref().map(|c| &c.id) == Some(&self.cluster);
        if active {
            "OperatorHub".into()
        } else {
            let name = ConnectionManager::try_global(cx)
                .map(|m| m.read(cx).display_name(&self.cluster))
                .unwrap_or_default();
            format!("OperatorHub · {name}").into()
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Store.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(request(&self.cluster))
    }
}

impl Render for OperatorHubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        // Fetches when missing or stale.
        let (loading, error) = match Olm::global(cx) {
            Some(olm) => olm.update(cx, |olm, cx| {
                let state = olm.hub(&self.cluster, false, cx);
                (state.loading, state.error.clone())
            }),
            None => (false, None),
        };
        let availability = Olm::global(cx)
            .map(|o| o.read(cx).availability(&self.cluster, cx))
            .unwrap_or(Availability::NotConnected);
        let packages = self.packages(cx);
        if let Some(packages) = &packages {
            self.refresh_shown(packages, cx);
        }
        if let Some(name) = cx
            .try_global::<PendingPackages>()
            .and_then(|p| p.0.get(&self.cluster).cloned())
            && packages.is_some()
        {
            cx.default_global::<PendingPackages>()
                .0
                .remove(&self.cluster);
            if let Some(index) = self.shown.iter().position(|p| p.name == name) {
                self.select_index(index, window, cx);
                self.details_open = true;
            }
        }
        // The selection follows the filter: the first match when it's filtered out.
        if packages.is_some()
            && self
                .selected
                .as_ref()
                .is_none_or(|k| !self.shown.iter().any(|p| &p.key() == k))
        {
            self.selected = self.shown.first().map(|p| p.key());
            self.description_open = false;
        }
        let catalogs = packages
            .as_ref()
            .map(|p| {
                p.iter()
                    .map(|x| {
                        x.key()
                            .rsplit_once('/')
                            .map(|(c, _)| c.to_string())
                            .unwrap_or_default()
                    })
                    .collect::<BTreeSet<_>>()
                    .len()
            })
            .unwrap_or(0);
        let installed_count = self.installed(cx).len();
        let summary = match (&packages, loading) {
            (Some(p), _) => format!(
                "{} packages from {catalogs} {} · {installed_count} installed",
                p.len(),
                if catalogs == 1 { "catalog" } else { "catalogs" }
            ),
            (None, true) => "Listing the catalogs' packages…".into(),
            (None, false) => String::new(),
        };
        let weak = cx.entity().downgrade();
        let sort = self.sort;
        let sort_menu = MenuButton::new("hub-sort")
            .ghost()
            .compact()
            .child(
                h_flex()
                    .gap(u(4.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_muted)
                    .child(match sort {
                        Sort::Relevance => "Sort: Relevance",
                        Sort::Name => "Sort: A–Z",
                    })
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |menu, _, _| {
                let pick = |value: Sort| {
                    let weak = weak.clone();
                    move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                        weak.update(cx, |this, cx| {
                            this.sort = value;
                            cx.notify();
                        })
                        .ok();
                    }
                };
                menu.item(
                    PopupMenuItem::new("Relevance")
                        .checked(sort == Sort::Relevance)
                        .on_click(pick(Sort::Relevance)),
                )
                .item(
                    PopupMenuItem::new("A–Z")
                        .checked(sort == Sort::Name)
                        .on_click(pick(Sort::Name)),
                )
            });
        let toolbar = h_flex()
            .flex_none()
            .h(u(40.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(Icon::new(IconName::Store).size(14.0).color(colors.accent))
            .child(div().font_weight(FontWeight::MEDIUM).child("OperatorHub"))
            .child(div().text_color(colors.text_dim).child("·"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colors.text_dim)
                    .child(summary),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w(u(260.0))
                    .h(u(sizes::CONTROL))
                    .px(u(8.0))
                    .gap(u(7.0))
                    .rounded(u(5.0))
                    .bg(colors.input_background)
                    .border_1()
                    .border_color(colors.border)
                    .child(
                        Icon::new(IconName::Search)
                            .size(12.0)
                            .color(colors.text_dim),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.search)
                                .appearance(false)
                                .text_size(u(12.5)),
                        ),
                    ),
            )
            .child(sort_menu)
            .child(
                kubyl_ui::IconButton::new("hub-reload", IconName::RefreshCw)
                    .icon_size(13.0)
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            );
        let body: AnyElement = match (&packages, availability) {
            (_, Availability::NoOlm) => widgets::empty(
                "OLM isn't installed on this cluster: there's no OperatorHub.",
                &colors,
            ),
            (_, Availability::NotConnected) => widgets::empty("Connecting…", &colors),
            (None, _) => match error {
                Some(err) => v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(u(10.0))
                    .p(u(24.0))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(22.0)
                            .color(colors.yellow),
                    )
                    .child(
                        div()
                            .max_w(u(560.0))
                            .text_color(colors.text_muted)
                            .child(err),
                    )
                    .child(
                        Button::new("hub-retry")
                            .icon(IconName::RefreshCw)
                            .label("Try again")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .into_any_element(),
                None => widgets::empty(
                    "Listing the catalogs' packages (the first time takes a few seconds)…",
                    &colors,
                ),
            },
            (Some(packages), _) => {
                let filters = self.render_filters(packages, cx);
                let columns = self.columns(window);
                let rows = self.shown.len().div_ceil(columns);
                let grid: AnyElement = if self.shown.is_empty() {
                    widgets::empty("No packages match.", &colors)
                } else {
                    uniform_list(
                        "hub-grid",
                        rows,
                        cx.processor(|this, range: Range<usize>, window, cx| {
                            this.render_rows(range, window, cx)
                        }),
                    )
                    .size_full()
                    .track_scroll(&self.scroll)
                    .into_any_element()
                };
                let details = self
                    .details_open
                    .then(|| self.selected_package())
                    .flatten()
                    .map(|p| self.render_details(p, cx));
                let width = self.width.clone();
                let entity = cx.weak_entity();
                h_flex()
                    .size_full()
                    .items_start()
                    .child(filters)
                    .child(
                        div()
                            .key_context(CONTEXT)
                            .track_focus(&self.focus)
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(
                                canvas(
                                    move |bounds, _, cx| {
                                        let measured = f32::from(bounds.size.width);
                                        let old = width.replace(measured);
                                        if (old - measured).abs() > 1.0 {
                                            let entity = entity.clone();
                                            cx.defer(move |cx| {
                                                entity.update(cx, |_, cx| cx.notify()).ok();
                                            });
                                        }
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(grid),
                    )
                    .children(details)
                    .into_any_element()
            }
        };
        let hints: Vec<(SharedString, SharedString)> = {
            let read_only = self.read_only(cx);
            let mut hints: Vec<(SharedString, SharedString)> = ActionRegistry::global(cx)
                .hints(CONTEXT)
                .into_iter()
                .filter(|(_, h)| !read_only || h.as_ref() != "Install…")
                .collect();
            hints.push(("/".into(), "Search".into()));
            hints
        };
        v_flex()
            .key_context("OperatorHubView")
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .on_action(cx.listener(|this, _: &PackageDetails, _, cx| {
                this.details_open = !this.details_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &InstallPackage, window, cx| this.install(window, cx)))
            .on_action(cx.listener(|this, _: &ReloadCatalogs, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                let focus = this.search.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &BlurSearch, window, cx| this.focus.focus(window, cx)))
            .on_action(
                cx.listener(|this, _: &NextPackage, window, cx| this.move_selection(1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &PreviousPackage, window, cx| {
                this.move_selection(-1, window, cx)
            }))
            .child(toolbar)
            .child(div().flex_1().min_h_0().flex().child(body))
            .child(kubyl_ui::KeyHints::new(hints))
    }
}
