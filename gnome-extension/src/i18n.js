// SPDX-License-Identifier: GPL-3.0-or-later

import Gio from 'gi://Gio';
import Gettext from 'gettext';

const DOMAIN = 'isle@zickrian.github.io';
const locale = Gio.File.new_for_uri(import.meta.url).get_parent().get_parent().get_child('locale');

// Some of the tab labels are translated when their modules are first imported.
Gettext.bindtextdomain(DOMAIN, locale.get_path());

export function gettext(message) {
    return message ? Gettext.dgettext(DOMAIN, message) : message;
}

/** Translate a sentence and insert its values without interpreting the values as formatting. */
export function format(message, ...values) {
    let index = 0;
    return gettext(message).replace(/%(?:(\d+)\$)?s/g,
        (placeholder, position) => String(values[position ? Number(position) - 1 : index++] ?? placeholder));
}
