use gtk::glib;
use gtk::prelude::*;
use gtk::{IconLookupFlags, IconTheme, Image, Menu, MenuBar, MenuItem, SeparatorMenuItem};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::thread;
use stray::message::menu::{MenuType, TrayMenu};
use stray::message::tray::StatusNotifierItem;
use stray::message::{NotifierItemCommand, NotifierItemMessage};
use stray::StatusNotifierWatcher;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

struct NotifierItem {
    item: StatusNotifierItem,
    menu: Option<TrayMenu>,
}

pub struct StatusNotifierWrapper {
    menu: stray::message::menu::MenuItem,
}

static STATE: Lazy<Mutex<HashMap<String, NotifierItem>>> = Lazy::new(|| Mutex::new(HashMap::new()));

impl StatusNotifierWrapper {
    fn into_menu_item(
        self,
        sender: mpsc::Sender<NotifierItemCommand>,
        notifier_address: String,
        menu_path: String,
    ) -> MenuItem {
        let item: Box<dyn AsRef<MenuItem>> = match self.menu.menu_type {
            MenuType::Separator => Box::new(SeparatorMenuItem::new()),
            MenuType::Standard => Box::new(MenuItem::with_label(self.menu.label.as_str())),
        };

        let item = (*item).as_ref().clone();

        {
            let sender = sender.clone();
            let notifier_address = notifier_address.clone();
            let menu_path = menu_path.clone();

            item.connect_activate(move |_item| {
                sender
                    .try_send(NotifierItemCommand::MenuItemClicked {
                        submenu_id: self.menu.id,
                        menu_path: menu_path.clone(),
                        notifier_address: notifier_address.clone(),
                    })
                    .unwrap();
            });
        };

        let submenu = Menu::new();
        if !self.menu.submenu.is_empty() {
            for submenu_item in self.menu.submenu.iter().cloned() {
                let submenu_item = StatusNotifierWrapper { menu: submenu_item };
                let submenu_item = submenu_item.into_menu_item(
                    sender.clone(),
                    notifier_address.clone(),
                    menu_path.clone(),
                );
                submenu.append(&submenu_item);
            }

            item.set_submenu(Some(&submenu));
        }

        item
    }
}

impl NotifierItem {
    fn get_icon(&self) -> Option<Image> {
        self.item.icon_theme_path.as_ref().map(|path| {
            let theme = IconTheme::new();
            theme.append_search_path(&path);
            let icon_name = self.item.icon_name.as_ref().unwrap();
            let icon_info = theme
                .lookup_icon(icon_name, 24, IconLookupFlags::empty())
                .expect("Failed to lookup icon info");

            Image::from_pixbuf(icon_info.load_icon().ok().as_ref())
        })
    }
}

fn main() {
    tracing_subscriber::fmt::init();

    let application = gtk::Application::new(
        Some("com.github.gtk-rs.examples.menu_bar_system"),
        Default::default(),
    );

    application.connect_activate(build_ui);

    application.run();
}

fn build_ui(application: &gtk::Application) {
    let window = gtk::ApplicationWindow::new(application);
    window.set_title("System menu bar");
    window.set_border_width(10);
    window.set_position(gtk::WindowPosition::Center);
    window.set_default_size(350, 70);

    let menu_bar = MenuBar::new();
    window.add(&menu_bar);
    let (sender, receiver) = mpsc::channel(32);
    let (cmd_tx, cmd_rx) = mpsc::channel(32);

    spawn_local_handler(menu_bar, receiver, cmd_tx);
    start_communication_thread(sender, cmd_rx);
    window.show_all();
}

fn spawn_local_handler(
    v_box: MenuBar,
    mut receiver: mpsc::Receiver<NotifierItemMessage>,
    cmd_tx: mpsc::Sender<NotifierItemCommand>,
) {
    let main_context = glib::MainContext::default();
    let future = async move {
        while let Some(item) = receiver.recv().await {
            let mut state = STATE.lock().unwrap();

            match item {
                NotifierItemMessage::Update {
                    address: id,
                    item,
                    menu,
                } => {
                    state.insert(id, NotifierItem { item: *item, menu });
                }
                NotifierItemMessage::Remove { address } => {
                    state.remove(&address);
                }
            }

            for child in v_box.children() {
                v_box.remove(&child);
            }

            for (address, notifier_item) in state.iter() {
                if let Some(icon) = notifier_item.get_icon() {
                    // Create the menu

                    let menu_item = MenuItem::new();
                    let menu_item_box = gtk::Box::default();
                    menu_item_box.add(&icon);
                    menu_item.add(&menu_item_box);

                    if let Some(tray_menu) = &notifier_item.menu {
                        let menu = Menu::new();
                        tray_menu
                            .submenus
                            .iter()
                            .map(|submenu| StatusNotifierWrapper {
                                menu: submenu.to_owned(),
                            })
                            .map(|item| {
                                let menu_path =
                                    notifier_item.item.menu.as_ref().unwrap().to_string();
                                let address = address.to_string();
                                item.into_menu_item(cmd_tx.clone(), address, menu_path)
                            })
                            .for_each(|item| menu.append(&item));

                        if !tray_menu.submenus.is_empty() {
                            menu_item.set_submenu(Some(&menu));
                        }
                    }
                    v_box.append(&menu_item);
                };

                v_box.show_all();
            }
        }
    };

    main_context.spawn_local(future);
}

fn start_communication_thread(
    sender: mpsc::Sender<NotifierItemMessage>,
    cmd_rx: mpsc::Receiver<NotifierItemCommand>,
) {
    thread::spawn(move || {
        let runtime = Runtime::new().expect("Failed to create tokio RT");

        runtime.block_on(async {
            let tray = StatusNotifierWatcher::new(cmd_rx).await.unwrap();
            let mut host = tray.create_notifier_host("MyHost").await.unwrap();

            while let Ok(message) = host.recv().await {
                sender
                    .send(message)
                    .await
                    .expect("failed to send message to UI");
            }

            host.destroy().await.unwrap();
        })
    });
}
