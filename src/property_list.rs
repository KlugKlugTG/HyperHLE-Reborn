/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Shared property-list decoding for bundle metadata and Foundation.
use plist::{Error, Value};
use quick_xml::{events::Event, Reader};
use std::io::Cursor;

pub(crate) fn read(data: &[u8]) -> Result<Value, Error> {
    Value::from_reader(Cursor::new(data)).or_else(|error| read_xml_prefix(data).ok_or(error))
}

pub(crate) fn read_xml(data: &[u8]) -> Result<Value, Error> {
    Value::from_reader_xml(data).or_else(|error| read_xml_prefix(data).ok_or(error))
}

/// Apple's parseXMLPropertyList returns after parsing the first XML element.
/// Some bundles contain extra text after that element. On failure, retry the
/// complete element with the same plist decoder, so invalid contents inside
/// the element still fail. Match XML tokens to handle comments and nesting.
/// https://github.com/apple-oss-distributions/CF/blob/main/CFPropertyList.c#L2194-L2225
fn read_xml_prefix(data: &[u8]) -> Option<Value> {
    let mut reader = Reader::from_reader(data);
    loop {
        match reader.read_event().ok()? {
            Event::Start(tag) => {
                reader.read_to_end(tag.name()).ok()?;
                break;
            }
            Event::Empty(_) => break,
            Event::Decl(_) | Event::DocType(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::Text(text) if text.as_ref().iter().all(u8::is_ascii_whitespace) => {}
            _ => return None,
        }
    }
    let end = usize::try_from(reader.buffer_position()).ok()?;
    if end >= data.len() {
        return None;
    }
    Value::from_reader_xml(&data[..end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_trailing_data_after_complete_xml_root() {
        let data = b"<?xml version='1.0'?><plist><dict><!-- </plist> --><key>k</key>\
                     <string>v</string></dict></plist>\nextra <invalid";
        for value in [read(data), read_xml(data)] {
            assert_eq!(
                value.unwrap().as_dictionary().unwrap()["k"].as_string(),
                Some("v")
            );
        }
    }

    #[test]
    fn rejects_invalid_contents_before_end_of_xml_root() {
        for data in [
            &b"<plist><dict><key>k</key></dict></plist>extra"[..],
            b"<plist><array></dict></plist>extra",
            b"<plist><array></array>extra",
        ] {
            assert!(read(data).is_err());
            assert!(read_xml(data).is_err());
        }
    }
}
