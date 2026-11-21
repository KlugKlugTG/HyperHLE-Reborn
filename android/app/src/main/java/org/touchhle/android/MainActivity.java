/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * Parts of this file are derived from SDL 2's Android project template, which
 * has a different license. Please see vendor/SDL/LICENSE.txt for details.
 */
package org.touchhle.android;

import android.app.Activity;
import android.content.ContentResolver;
import android.content.Intent;
import android.net.wifi.WifiManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.util.Log;

import org.libsdl.app.SDLActivity;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;

/**
 * A wrapper class over SDLActivity
 */

public class MainActivity extends SDLActivity {
    private static final String TAG = "touchHLE";

    // Public accessor for SDLActivity's protected static mSingleton, so
    // helper classes in this package (HostMedia) can reach the Activity.
    public static android.app.Activity getActivity() {
        return mSingleton;
    }

    // Keeps Wi-Fi multicast packets flowing while the app runs. Without this
    // the Android Wi-Fi driver filters mDNS (224.0.0.251:5353), breaking
    // Bonjour/CFNetService local multiplayer discovery. Held for the process
    // lifetime; released when the activity is destroyed.
    private static android.net.wifi.WifiManager.MulticastLock multicastLock;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // Acquire the multicast lock here (not just declare it): Android
        // Wi-Fi drivers drop multicast unless the app holds it, which broke
        // Bonjour/NSNetService LAN discovery (GameKit, Gameloft games).
        try {
            android.net.wifi.WifiManager wm =
                    (android.net.wifi.WifiManager) getApplicationContext()
                            .getSystemService(android.content.Context.WIFI_SERVICE);
            if (wm != null && multicastLock == null) {
                multicastLock = wm.createMulticastLock("touchHLE_mdns");
                multicastLock.setReferenceCounted(false);
                multicastLock.acquire();
                Log.i(TAG, "Wi-Fi multicast lock acquired (mDNS discovery enabled)");
            }
        } catch (Exception e) {
            Log.w(TAG, "Couldn't acquire Wi-Fi multicast lock", e);
        }
    }

    @Override
    protected void onDestroy() {
        try {
            if (multicastLock != null) {
                if (multicastLock.isHeld()) {
                    multicastLock.release();
                }
                multicastLock = null;
            }
        } catch (Exception e) {
            Log.w(TAG, "Couldn't release Wi-Fi multicast lock", e);
        }
        super.onDestroy();
    }

    // Message ID sent from the Rust app picker (see window.rs) to open the
    // .ipa file picker. Must match window.rs ADD_IPA_COMMAND.
    private static final int MSG_ADD_IPA = 0x8000;

    // Message ID sent from the Rust WebView bridge (see android_web_view.rs)
    // to notify the emulated app that a page finished loading in a real
    // WebView overlay. Payload packs (overlay id, action) into one long.

    // Request codes for system document pickers started by this activity.
    private static final int REQUEST_ADD_IPA = 1;
    private static final int REQUEST_OPEN_APPS_FOLDER = 2;

    // =====================================================================
    // Real WebView overlay support (called from Rust via JNI; see
    // src/android_web_view.rs). Each UIWebView in the emulated app can
    // display a genuine Android WebView layered on top of the SDL surface,
    // with real page loading (HTML/CSS/JS), link navigation, back/forward
    // and JavaScript evaluation.
    // =====================================================================
    private static final java.util.HashMap<Integer, android.webkit.WebView> webOverlays =
            new java.util.HashMap<Integer, android.webkit.WebView>();
    private static int nextWebOverlayId = 1;

    // Open a URL in the system browser (or the app that owns the scheme,
    // e.g. a mailto: handler). Called from Rust via JNI (see
    // src/android_web_view.rs) when the emulated app opens a URL link
    // (UIApplication openURL: / OpenAL-style URL launching).
    public static int openExternalUrl(final String url) {
        android.app.Activity act = mSingleton;
        if (act == null || url == null || url.isEmpty()) {
            return -1;
        }
        try {
            android.content.Intent i = new android.content.Intent(
                    android.content.Intent.ACTION_VIEW);
            i.setData(android.net.Uri.parse(url));
            // For content:// URIs from our DocumentsProvider the receiving
            // file manager needs read/write permission granted explicitly.
            if (url != null && url.startsWith("content://")) {
                i.addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION
                        | android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
                // Try to give a MIME type so the system can find a handler.
                // DocumentsProvider directories use vnd.android.document/directory.
                // Using setDataAndType instead of setData alone helps the
                // chooser on some OEMs.
                try {
                    i.setDataAndType(android.net.Uri.parse(url), "vnd.android.document/directory");
                    i.addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION
                            | android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
                } catch (Throwable ignore) {
                    i.setData(android.net.Uri.parse(url));
                }
            }
            i.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK);
            act.startActivity(i);
        } catch (Throwable t) {
            Log.e(TAG, "openExternalUrl failed for " + url, t);
            return -1;
        }
        return 0;
    }

    // Open a content:// URI from our DocumentsProvider in the system file
    // manager. This is used for the "File manager" button on Android.
    // Unlike openExternalUrl it always grants URI permissions and uses a
    // chooser so the user can pick a file manager if there are multiple.
    public static int openContentUri(final String uriStr) {
        android.app.Activity act = mSingleton;
        if (act == null || uriStr == null || uriStr.isEmpty()) {
            return -1;
        }
        // ACTION_VIEW with a directory MIME type is not consistently handled
        // by Android file managers (many devices have no matching activity).
        // Use the Storage Access Framework's tree picker instead; it is backed
        // by DocumentsProvider and works even when no third-party file manager
        // is installed. EXTRA_INITIAL_URI asks Android 8+ to start in the
        // touchHLE apps directory.
        try {
            final android.net.Uri uri = android.net.Uri.parse(uriStr);
            final android.content.Intent intent = new android.content.Intent(
                    android.content.Intent.ACTION_OPEN_DOCUMENT_TREE);
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
                intent.putExtra(android.provider.DocumentsContract.EXTRA_INITIAL_URI, uri);
            }
            // ACTION_OPEN_DOCUMENT_TREE is an activity-for-result contract.
            // Start it on the UI thread and use startActivityForResult rather
            // than a fire-and-forget startActivity; some Android versions do
            // not present the picker reliably without the result contract.
            act.runOnUiThread(new Runnable() {
                @Override
                public void run() {
                    try {
                        act.startActivityForResult(intent, REQUEST_OPEN_APPS_FOLDER);
                    } catch (Throwable t) {
                        Log.e(TAG, "Couldn't launch apps folder picker for " + uri, t);
                    }
                }
            });
            return 0;
        } catch (Throwable t) {
            Log.e(TAG, "openContentUri tree picker setup failed for " + uriStr, t);
            return -1;
        }
    }

    // Create a new overlay WebView loading `url` (may be empty to create it
    // blank). x/y/w/h are in window pixels; w or h <= 0 means "match the
    // window" on that axis. Returns the overlay id (always > 0).
    public static int showWebOverlay(final String url, final int x, final int y,
                                     final int w, final int h) {
        final int id = nextWebOverlayId++;
        mSingleton.runOnUiThread(new Runnable() {
            public void run() {
                createWebOverlay(id, url, x, y, w, h);
            }
        });
        return id;
    }

    private static void createWebOverlay(int id, String url, int x, int y, int w, int h) {
        android.app.Activity act = mSingleton;
        if (act == null) return;
        try {
            android.webkit.WebView wv = new android.webkit.WebView(act);
            android.webkit.WebSettings ws = wv.getSettings();
            ws.setJavaScriptEnabled(true);
            ws.setDomStorageEnabled(true);
            ws.setLoadWithOverviewMode(true);
            ws.setUseWideViewPort(true);
            ws.setBuiltInZoomControls(true);
            ws.setDisplayZoomControls(false);
            ws.setSupportZoom(true);
            ws.setMediaPlaybackRequiresUserGesture(false);
            wv.setWebChromeClient(new android.webkit.WebChromeClient());
            wv.setWebViewClient(new android.webkit.WebViewClient() {
                @Override
                public void onPageFinished(android.webkit.WebView view, String u) {
                    // The emulated side already fires webViewDidFinishLoad:
                    // on a short timer after the load starts, so nothing
                    // needs to be sent back into native code here.
                }
            });
            // Transparent background so the emulated app shows through before
            // the page paints.
            wv.setBackgroundColor(android.graphics.Color.TRANSPARENT);

            android.view.ViewGroup content =
                    (android.view.ViewGroup) act.findViewById(android.R.id.content);
            int width = w > 0 ? w : android.view.ViewGroup.LayoutParams.MATCH_PARENT;
            int height = h > 0 ? h : android.view.ViewGroup.LayoutParams.MATCH_PARENT;
            android.widget.FrameLayout.LayoutParams lp =
                    new android.widget.FrameLayout.LayoutParams(width, height);
            content.addView(wv, lp);
            wv.setTranslationX(x);
            wv.setTranslationY(y);
            content.bringChildToFront(wv);

            if (url != null && !url.isEmpty()) {
                wv.loadUrl(url);
            }
            webOverlays.put(id, wv);
        } catch (Throwable t) {
            Log.e(TAG, "createWebOverlay failed", t);
        }
    }

    // Navigate an existing overlay to a new URL.
    public static void navigateWebOverlay(final int id, final String url) {
        withWebOverlay(id, new Runnable() {
            public void run() {
                webOverlays.get(id).loadUrl(url);
            }
        });
    }

    // Load raw data (e.g. loadHTMLString:) into an existing overlay.
    public static void loadDataWebOverlay(final int id, final String data,
                                          final String mime) {
        withWebOverlay(id, new Runnable() {
            public void run() {
                webOverlays.get(id).loadDataWithBaseURL(null, data,
                        mime != null ? mime : "text/html", "utf-8", null);
            }
        });
    }

    // Move/resize an existing overlay (window pixels).
    public static void setWebOverlayBounds(final int id, final int x, final int y,
                                           final int w, final int h) {
        withWebOverlay(id, new Runnable() {
            public void run() {
                android.webkit.WebView wv = webOverlays.get(id);
                android.view.ViewGroup.LayoutParams lp0 = wv.getLayoutParams();
                int width = w > 0 ? w : lp0.width;
                int height = h > 0 ? h : lp0.height;
                android.widget.FrameLayout.LayoutParams lp;
                if (lp0 instanceof android.widget.FrameLayout.LayoutParams) {
                    lp = (android.widget.FrameLayout.LayoutParams) lp0;
                    lp.width = width;
                    lp.height = height;
                } else {
                    lp = new android.widget.FrameLayout.LayoutParams(width, height);
                    wv.setLayoutParams(lp);
                }
                wv.setTranslationX(x);
                wv.setTranslationY(y);
                wv.requestLayout();
            }
        });
    }

    // Remove and destroy an overlay.
    public static void hideWebOverlay(final int id) {
        android.app.Activity act = mSingleton;
        if (act == null) return;
        act.runOnUiThread(new Runnable() {
            public void run() {
                android.webkit.WebView wv = webOverlays.remove(id);
                if (wv == null) return;
                try {
                    android.view.ViewGroup parent = (android.view.ViewGroup) wv.getParent();
                    if (parent != null) parent.removeView(wv);
                    wv.stopLoading();
                    wv.destroy();
                } catch (Throwable t) {
                    Log.e(TAG, "hideWebOverlay failed", t);
                }
            }
        });
    }

    public static void goBackWebOverlay(final int id) {
        withWebOverlay(id, new Runnable() {
            public void run() {
                if (webOverlays.get(id).canGoBack()) webOverlays.get(id).goBack();
            }
        });
    }

    public static void goForwardWebOverlay(final int id) {
        withWebOverlay(id, new Runnable() {
            public void run() {
                if (webOverlays.get(id).canGoForward()) webOverlays.get(id).goForward();
            }
        });
    }

    public static boolean canGoBackWebOverlay(final int id) {
        return withWebOverlayResult(id, new java.util.concurrent.Callable<Boolean>() {
            public Boolean call() {
                return webOverlays.get(id).canGoBack();
            }
        }, Boolean.FALSE);
    }

    public static boolean canGoForwardWebOverlay(final int id) {
        return withWebOverlayResult(id, new java.util.concurrent.Callable<Boolean>() {
            public Boolean call() {
                return webOverlays.get(id).canGoForward();
            }
        }, Boolean.FALSE);
    }

    // Evaluate JavaScript synchronously (called from the SDL thread; blocks
    // the caller for up to ~5s until the UI thread produces the result).
    public static String evalJsWebOverlay(final int id, final String script) {
        return withWebOverlayResult(id, new java.util.concurrent.Callable<String>() {
            public String call() {
                final java.util.concurrent.CountDownLatch latch =
                        new java.util.concurrent.CountDownLatch(1);
                final String[] result = new String[]{""};
                webOverlays.get(id).evaluateJavascript(script,
                        new android.webkit.ValueCallback<String>() {
                            public void onReceiveValue(String value) {
                                result[0] = value != null ? value : "";
                                latch.countDown();
                            }
                        });
                try {
                    latch.await(5, java.util.concurrent.TimeUnit.SECONDS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
                return result[0];
            }
        }, "");
    }

    public static void stopLoadingWebOverlay(final int id) {
        withWebOverlay(id, new Runnable() {
            public void run() {
                webOverlays.get(id).stopLoading();
            }
        });
    }

    private static void withWebOverlay(final int id, final Runnable r) {
        android.app.Activity act = mSingleton;
        if (act == null) return;
        act.runOnUiThread(new Runnable() {
            public void run() {
                if (webOverlays.containsKey(id)) r.run();
            }
        });
    }

    // Run a Callable that touches a WebView on the UI thread and block the
    // SDL thread until the result is available (or timeout).
    @SuppressWarnings("unchecked")
    private static <T> T withWebOverlayResult(final int id,
                                              final java.util.concurrent.Callable<T> c,
                                              T fallback) {
        final java.util.concurrent.CountDownLatch latch =
                new java.util.concurrent.CountDownLatch(1);
        final Object[] out = new Object[1];
        final Throwable[] err = new Throwable[1];
        Runnable body = new Runnable() {
            public void run() {
                try {
                    if (webOverlays.containsKey(id)) out[0] = c.call();
                } catch (Throwable t) {
                    err[0] = t;
                } finally {
                    latch.countDown();
                }
            }
        };
        android.app.Activity act = mSingleton;
        if (act == null) return fallback;
        if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
            body.run();
        } else {
            act.runOnUiThread(body);
        }
        try {
            latch.await(5, java.util.concurrent.TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        if (err[0] != null) {
            Log.e(TAG, "webOverlay call failed", err[0]);
            return fallback;
        }
        if (out[0] == null) return fallback;
        return (T) out[0];
    }

    @Override
    protected String[] getLibraries() {
        return new String[]{
            "SDL2",
            "touchHLE"
        };
    }

    @Override
    protected boolean onUnhandledMessage(int message, Object data) {
        if (message == MSG_ADD_IPA) {
            // The message arrives on SDL's native thread; the file picker
            // must be started from the UI thread.
            runOnUiThread(new Runnable() {
                public void run() {
                    openIpaPicker();
                }
            });
            return true;
        }
        // MSG_WEB_OVERLAY (0x8001) is reserved: native code notifies the
        // emulated app about page loads itself (via NSTimer), so there is
        // nothing to handle here yet.
        return super.onUnhandledMessage(message, data);
    }

    private void openIpaPicker() {
        Intent intent = new Intent(Intent.ACTION_GET_CONTENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        intent.putExtra(Intent.EXTRA_MIME_TYPES, new String[]{
            "application/octet-stream", "application/zip",
            "application/x-zip-compressed"});
        try {
            startActivityForResult(
                Intent.createChooser(intent, "Add game (.ipa)"),
                REQUEST_ADD_IPA);
        } catch (Exception e) {
            Log.e(TAG, "Couldn't open file picker", e);
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_ADD_IPA || resultCode != Activity.RESULT_OK
                || data == null || data.getData() == null) {
            return;
        }
        Uri uri = data.getData();
        String name = displayName(uri);
        if (name == null || name.isEmpty()) {
            name = "game.ipa";
        }
        if (!name.toLowerCase().endsWith(".ipa")) {
            name += ".ipa";
        }
        copyIpa(uri, name);
    }

    private String displayName(Uri uri) {
        ContentResolver resolver = getContentResolver();
        Cursor cursor = null;
        try {
            cursor = resolver.query(uri, null, null, null, null);
            if (cursor != null && cursor.moveToFirst()) {
                int idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (idx >= 0 && !cursor.isNull(idx)) {
                    return cursor.getString(idx);
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "Couldn't query display name", e);
        } finally {
            if (cursor != null) {
                cursor.close();
            }
        }
        String last = uri.getLastPathSegment();
        return last == null ? null : last.substring(last.lastIndexOf('/') + 1);
    }

    private void copyIpa(Uri uri, String name) {
        // HyperHLE lists apps from this directory (see APPS_DIR in paths.rs);
        // it matches SDL_AndroidGetExternalStoragePath() on Android.
        File appsDir = new File(getExternalFilesDir(null), "HyperHLE_apps");
        if (!appsDir.exists() && !appsDir.mkdirs()) {
            Log.e(TAG, "Couldn't create " + appsDir);
            return;
        }
        InputStream in = null;
        FileOutputStream out = null;
        try {
            in = getContentResolver().openInputStream(uri);
            if (in == null) {
                Log.e(TAG, "Couldn't open " + uri);
                return;
            }
            out = new FileOutputStream(new File(appsDir, name));
            byte[] buffer = new byte[65536];
            int read;
            while ((read = in.read(buffer)) >= 0) {
                out.write(buffer, 0, read);
            }
            out.flush();
            Log.i(TAG, "Added game to " + appsDir + ": " + name);
        } catch (IOException | SecurityException e) {
            Log.e(TAG, "Couldn't copy .ipa file: " + name, e);
        } finally {
            if (in != null) {
                try {
                    in.close();
                } catch (IOException e) {
                    // Nothing to do.
                }
            }
            if (out != null) {
                try {
                    out.close();
                } catch (IOException e) {
                    // Nothing to do.
                }
            }
        }
    }
}
