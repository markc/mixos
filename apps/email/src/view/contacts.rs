// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The contacts surface: an account's selected address book's contacts in
//! the centre pane (with the text filter the server runs over name, email
//! and company) and the open contact's detail in the reader (the address
//! books themselves are the tree's).

use egui::{RichText, ScrollArea, Ui};
use jmap::Contact;
use toolkit::{Icon, icons, panel};

use super::UiEvent;
use crate::state::AccountView;

/// The centre pane's contact list, with its filter.
pub fn list(ui: &mut Ui, view: &AccountView, events: &mut Vec<UiEvent>) {
    let title = view
        .contacts
        .books
        .iter()
        .find(|book| Some(book.id.as_str()) == view.contacts.selected.as_deref())
        .map(|book| book.name.clone())
        .unwrap_or_else(|| crate::label("contacts"));
    panel::group(ui, "contacts", &[title.as_str()], |ui, _| {
        let mut filter = view.contacts.filter.clone();
        let search = toolkit::field::search(
            ui,
            &mut filter,
            &crate::label("contact-search"),
            ui.available_width(),
        );
        if search.changed() {
            events.push(UiEvent::ContactFilter(view.id().to_owned(), filter));
        }
        if view.contacts.busy {
            ui.weak(crate::label("loading"));
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if view.contacts.contacts.is_empty() && !view.contacts.busy {
                ui.weak(crate::label("empty"));
            }
            for (index, contact) in view.contacts.contacts.iter().enumerate() {
                row(ui, index, contact, view, events);
            }
        });
    });
}

/// One contact row: name, email.
fn row(
    ui: &mut Ui,
    index: usize,
    contact: &Contact,
    view: &AccountView,
    events: &mut Vec<UiEvent>,
) {
    let selected = view.contacts.open == Some(index);
    let weak = ui.visuals().weak_text_color();
    egui::Sides::new().show(
        ui,
        |ui| {
            ui.add(icons::image_of(ui.ctx(), Icon::Contact, 12.0, weak));
            let text = if selected {
                RichText::new(contact.display_name()).strong()
            } else {
                RichText::new(contact.display_name())
            };
            if toolkit::selectable(ui, selected, text).clicked() {
                events.push(UiEvent::SelectContact(view.id().to_owned(), index));
            }
        },
        |ui| {
            ui.label(RichText::new(contact.display_email()).weak());
        },
    );
}

/// The reader: the open contact's detail.
pub fn reader(ui: &mut Ui, view: &AccountView) {
    let Some(contact) = view
        .contacts
        .open
        .and_then(|index| view.contacts.contacts.get(index))
    else {
        ui.weak(crate::label("select-contact"));
        return;
    };
    panel::section_label(ui, &contact.display_name());
    detail(ui, "email-line", Some(contact.display_email()).as_deref());
    detail(ui, "phone-line", contact.phone.as_deref());
    detail(ui, "company-line", contact.company.as_deref());
    detail(
        ui,
        "address-book-line",
        view.contacts
            .books
            .iter()
            .find(|book| book.id == contact.address_book_id)
            .map(|book| book.name.as_str()),
    );
    toolkit::bars::hairline(ui);
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        // Extra addresses out of the JSContact card.
        if let Some(emails) = contact.data.get("emails").and_then(|e| e.as_object()) {
            for entry in emails.values() {
                if let Some(address) = entry.get("address").and_then(|a| a.as_str())
                    && address != contact.display_email()
                {
                    ui.label(RichText::new(address).weak());
                }
            }
        }
    });
}

/// One detail line: the label with the value, when the value is set.
fn detail(ui: &mut Ui, label: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        ui.label(RichText::new(crate::label_with(label, &[("value", value)])).weak());
    }
}
