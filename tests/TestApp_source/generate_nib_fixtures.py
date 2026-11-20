# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Regenerate synthetic nibs for lookup, placeholder and control tests."""

from pathlib import Path
import plistlib


def generate():
    uid = plistlib.UID
    objects = [
        "$null",
        {"$class": uid(5), "UIProxiedObjectIdentifier": uid(9)},
        {"$class": uid(5), "UIProxiedObjectIdentifier": uid(10)},
        {"$class": uid(6)},
        {"$class": uid(7), "NS.objects": [uid(1), uid(2), uid(3)]},
        {"$classname": "UIProxyObject", "$classes": ["UIProxyObject", "NSObject"]},
        {"$classname": "NibTopLevelProbe", "$classes": ["NibTopLevelProbe", "NSObject"]},
        {"$classname": "NSArray", "$classes": ["NSArray", "NSObject"]},
        {"$class": uid(7), "NS.objects": []},
        "IBFilesOwner",
        "IBFirstResponder",
    ]
    archive = {
        "$archiver": "NSKeyedArchiver",
        "$version": 100000,
        "$objects": objects,
        "$top": {
            "UINibObjectsKey": uid(4),
            "UINibTopLevelObjectsKey": uid(4),
            "UINibConnectionsKey": uid(8),
            "UINibVisibleWindowsKey": uid(8),
        },
    }
    output = Path(__file__).resolve().parent.parent / "TestApp.app" / "ProxyTopLevel.nib"
    output.write_bytes(plistlib.dumps(archive, fmt=plistlib.FMT_BINARY, sort_keys=False))


def generate_cell_controls():
    uid = plistlib.UID
    objects = [
        "$null",
        {"$class": uid(8), "UIFrame": uid(17), "UISubviews": uid(6), "UIContentView": uid(2)},
        {"$class": uid(9), "UIFrame": uid(17), "UISubviews": uid(7)},
        {"$class": uid(10), "UIFrame": uid(18), "UISwitchOn": False},
        {"$class": uid(11), "UIFrame": uid(19), "UIValue": 0.5},
        {"$class": uid(12), "UIFrame": uid(20), "UIText": uid(21), "UIFont": uid(22), "UITextColor": uid(23)},
        {"$class": uid(13), "NS.objects": [uid(2)]},
        {"$class": uid(13), "NS.objects": [uid(3), uid(4), uid(5)]},
        {"$classname": "UITableViewCell", "$classes": ["UITableViewCell", "UIView", "NSObject"]},
        {"$classname": "UITableViewCellContentView", "$classes": ["UITableViewCellContentView", "UIView", "NSObject"]},
        {"$classname": "UISwitch", "$classes": ["UISwitch", "UIControl", "UIView", "NSObject"]},
        {"$classname": "UISlider", "$classes": ["UISlider", "UIControl", "UIView", "NSObject"]},
        {"$classname": "UITextField", "$classes": ["UITextField", "UIControl", "UIView", "NSObject"]},
        {"$classname": "NSArray", "$classes": ["NSArray", "NSObject"]},
        {"$class": uid(13), "NS.objects": [uid(1), uid(2), uid(3), uid(4), uid(5)]},
        {"$class": uid(13), "NS.objects": [uid(1)]},
        {"$class": uid(13), "NS.objects": []},
        "{{0, 0}, {320, 132}}",
        "{{200, 6}, {94, 27}}",
        "{{20, 45}, {280, 31}}",
        "{{20, 90}, {280, 25}}",
        "Guest",
        {"$class": uid(24), "UIFontName": uid(26), "UIFontPointSize": 17.0},
        {"$class": uid(25), "UIWhite": 0.0, "UIAlpha": 1.0},
        {"$classname": "UIFont", "$classes": ["UIFont", "NSObject"]},
        {"$classname": "UIColor", "$classes": ["UIColor", "NSObject"]},
        "Helvetica",
    ]
    archive = {
        "$archiver": "NSKeyedArchiver",
        "$version": 100000,
        "$objects": objects,
        "$top": {
            "UINibObjectsKey": uid(14),
            "UINibTopLevelObjectsKey": uid(15),
            "UINibConnectionsKey": uid(16),
            "UINibVisibleWindowsKey": uid(16),
        },
    }
    output = Path(__file__).resolve().parent.parent / "TestApp.app" / "CellControls.nib"
    output.write_bytes(plistlib.dumps(archive, fmt=plistlib.FMT_BINARY, sort_keys=False))


def generate_lookup_fixtures():
    directory = Path(__file__).resolve().parent.parent / "TestApp.app" / "TestResources.bundle"
    for name in [
        "NibSearchFullController.nib",
        "NibSearchPadView~ipad.nib",
        "NibSearchView.nib",
        "NibSearchViewController.nib",
    ]:
        # These resources are only searched by name; they are never loaded.
        (directory / name).write_bytes(b"Fixture for nibName selection only; not an archive.\n")


if __name__ == "__main__":
    generate()
    generate_cell_controls()
    generate_lookup_fixtures()
