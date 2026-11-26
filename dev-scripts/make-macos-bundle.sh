#!/bin/sh
set -e

# Creates the .app bundle containing the basic set of files needed for touchHLE
# to run. Also adds an icon and metadata similar to the Android APK.

if [[ $# == 3 ]]; then
    PATH_TO_BINARY="$1"
    VERSION="$2"
    BRANDING="$3"
    shift 3

    if [[ "x$BRANDING" == "x" ]]; then
        APP_NAME=touchHLE
        ICON_NAME=icon
    elif [[ "$BRANDING" == "UNOFFICIAL" ]]; then
        # UNOFFICIAL now uses the regular icon (no icon_unofficial.png)
        APP_NAME="touchHLE $BRANDING"
        ICON_NAME=icon
        VERSION="$VERSION $BRANDING"
    else
        APP_NAME="touchHLE $BRANDING"
        ICON_NAME="icon_$(echo "$BRANDING" | tr 'A-Z' 'a-z')"
        VERSION="$VERSION $BRANDING"
    fi
    rm -rf "$APP_NAME.app"
    mkdir -p "$APP_NAME.app"/Contents/MacOS "$APP_NAME.app"/Contents/Resources
    cp $PATH_TO_BINARY "$APP_NAME.app"/Contents/MacOS/touchHLE
    cp -r ../touchHLE_dylibs "$APP_NAME.app"/Contents/Resources/
    cp -r ../touchHLE_fonts "$APP_NAME.app"/Contents/Resources/
    cp -r ../touchHLE_default_options.txt "$APP_NAME.app"/Contents/Resources/
    cp ../touchHLE_wallpaper.png "$APP_NAME.app"/Contents/Resources/
    cp ../res/"$ICON_NAME.icns" "$APP_NAME.app"/Contents/Resources/

    plutil -create xml1 "$APP_NAME.app"/Contents/Info.plist
    plutil -insert CFBundleName -string "$APP_NAME" "$APP_NAME.app"/Contents/Info.plist
    plutil -insert CFBundleDisplayName -string "$APP_NAME" "$APP_NAME.app"/Contents/Info.plist
    plutil -insert CFBundleShortVersionString -string "$VERSION" "$APP_NAME.app"/Contents/Info.plist
    plutil -insert CFBundleExecutable -string touchHLE "$APP_NAME.app"/Contents/Info.plist
    plutil -insert CFBundleIconFile -string "$ICON_NAME" "$APP_NAME.app"/Contents/Info.plist
    plutil -insert NSHighResolutionCapable -bool YES "$APP_NAME.app"/Contents/Info.plist
    plutil -insert NSMicrophoneUsageDescription -string \
        "HyperHLE needs microphone access for games that record sound." \
        "$APP_NAME.app"/Contents/Info.plist
else
    echo "Incorrect usage."
    exit 1
fi
