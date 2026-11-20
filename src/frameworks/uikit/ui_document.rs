/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `UIDocument`.
//!
//! `open` / `revert` load the file and hand its `NSData` to the subclass's
//! `loadFromContents:ofType:error:`; `save` asks the subclass for
//! `contentsForType:error:` and writes it to the URL. Everything runs
//! synchronously and the completion block is called before the method
//! returns (real UIKit calls it later, on the main queue).

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string::from_rust_string;
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::mem::{ConstPtr, MutPtr};
use crate::objc::{
    autorelease, id, msg, msg_class, msg_super, nil, objc_classes, release, retain, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;

// MARK: - Constants

/// `UIDocumentState` (bit flags)
pub type UIDocumentState = NSUInteger;
pub const UIDocumentStateNormal: UIDocumentState = 0;
pub const UIDocumentStateClosed: UIDocumentState = 1 << 0;
pub const UIDocumentStateInConflict: UIDocumentState = 1 << 1;
pub const UIDocumentStateSavingError: UIDocumentState = 1 << 2;
pub const UIDocumentStateEditingDisabled: UIDocumentState = 1 << 3;
pub const UIDocumentStateProgressAvailable: UIDocumentState = 1 << 4;

/// `UIDocumentSaveOperation`
pub type UIDocumentSaveOperation = NSInteger;
pub const UIDocumentSaveForCreating: UIDocumentSaveOperation = 0;
pub const UIDocumentSaveForOverwriting: UIDocumentSaveOperation = 1;

/// `UIDocumentChangeKind`
pub type UIDocumentChangeKind = NSInteger;
pub const UIDocumentChangeDone: UIDocumentChangeKind = 0;
pub const UIDocumentChangeUndone: UIDocumentChangeKind = 1;
pub const UIDocumentChangeRedone: UIDocumentChangeKind = 2;
pub const UIDocumentChangeCleared: UIDocumentChangeKind = 3;

pub const UIDocumentStateChangedNotification: &str = "UIDocumentStateChangedNotification";

pub const CONSTANTS: ConstantExports = &[(
    "_UIDocumentStateChangedNotification",
    HostConstant::NSString(UIDocumentStateChangedNotification),
)];

// MARK: - Host object

pub struct UIDocumentHostObject {
    file_url: id,
    /// Between a successful `open` and `close`.
    open: bool,
    /// Net `updateChangeCount:` calls since the last save/revert/open.
    change_count: i32,
    /// The last save failed.
    saving_error: bool,
    editing_disabled: bool,
    /// Retained; only stored, never created here.
    undo_manager: id,
}

// Явная реализация трейта для устранения ошибки E0277
impl HostObject for UIDocumentHostObject {}

impl Default for UIDocumentHostObject {
    fn default() -> Self {
        Self {
            file_url: nil,
            open: false,
            change_count: 0,
            saving_error: false,
            editing_disabled: false,
            undo_manager: nil,
        }
    }
}

// MARK: - Block helpers

/// The invoke function of an Objective-C block (ARM32 layout:
/// isa, flags, reserved, invoke, ...). `None` for nil or a NULL invoke.
fn block_invoke(env: &mut Environment, block: id) -> Option<GuestFunction> {
    if block == nil {
        return None;
    }
    // Явно указываем ConstPtr::<u32>, чтобы компилятор не гадал о параметрах
    let invoke_addr: u32 = env.mem.read(ConstPtr::<u32>::from_bits(block.to_bits() + 12));
    if invoke_addr == 0 {
        log!("UIDocument: block {:?} has a NULL invoke pointer, ignoring", block);
        return None;
    }
    Some(GuestFunction::from_addr_with_thumb_bit(invoke_addr))
}

/// Calls a `void (^)(BOOL)` block.
fn call_bool_block(env: &mut Environment, block: id, arg: bool) {
    if let Some(invoke) = block_invoke(env, block) {
        let _: () = invoke.call_from_host(env, (block, arg as u32));
    }
}

/// Calls a `void (^)(void)` block.
fn call_void_block(env: &mut Environment, block: id) {
    if let Some(invoke) = block_invoke(env, block) {
        let _: () = invoke.call_from_host(env, (block,));
    }
}

fn responds_to(env: &mut Environment, obj: id, selector: &str) -> bool {
    match env.objc.lookup_selector(selector) {
        Some(sel) => {
            let responds: bool = msg![env; obj respondsToSelector:sel];
            responds
        }
        None => false,
    }
}

/// A scratch `NSError **` for calls into subclass code. Free with
/// `env.mem.free(ptr.cast())`.
fn alloc_error_out(env: &mut Environment) -> MutPtr<id> {
    env.mem.alloc_and_write(nil)
}

// MARK: - Load / save

/// Default `-readFromURL:error:`: read the file and give its data to the
/// subclass's `-loadFromContents:ofType:error:`.
fn load_from_url(env: &mut Environment, doc: id, url: id) -> bool {
    if url == nil {
        return false;
    }
    let path: id = msg![env; url path];
    if path == nil {
        return false;
    }
    let data: id = msg_class![env; NSData dataWithContentsOfFile:path];
    if data == nil {
        log!("UIDocument: could not read {:?}", url);
        return false;
    }
    if !responds_to(env, doc, "loadFromContents:ofType:error:") {
        // Real UIDocument raises here. Be lenient and report success.
        log!(
            "Warning: UIDocument subclass {:?} does not implement loadFromContents:ofType:error:",
            doc
        );
        return true;
    }
    let err = alloc_error_out(env);
    let ok: bool = msg![env; doc loadFromContents:data ofType:nil error:err];
    env.mem.free(err.cast());
    ok
}

/// Asks the subclass for its contents and writes them with
/// `-writeContents:toURL:forSaveOperation:originalContentsURL:error:`.
fn save_to_url(env: &mut Environment, doc: id, url: id, operation: NSInteger) -> bool {
    if url == nil {
        return false;
    }
    if !responds_to(env, doc, "contentsForType:error:") {
        // Nothing to write. Real UIDocument raises; be lenient (this is what
        // the old stub did).
        log!(
            "Warning: UIDocument subclass {:?} does not implement contentsForType:error:",
            doc
        );
        return true;
    }
    let err = alloc_error_out(env);
    let contents: id = msg![env; doc contentsForType:nil error:err];
    if contents == nil {
        env.mem.free(err.cast());
        log!("UIDocument: contentsForType:error: returned nil");
        return false;
    }
    let ok: bool = msg![env; doc writeContents:contents
                                         toURL:url
                              forSaveOperation:operation
                           originalContentsURL:nil
                                         error:err];
    env.mem.free(err.cast());
    ok
}

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);

    @implementation UIDocument: NSObject

    + (id)allocWithZone:(NSZonePtr)_zone {
        let host_object = Box::<UIDocumentHostObject>::default();
        env.objc.alloc_object(this, host_object, &mut env.mem)
    }

    - (id)initWithFileURL:(id)url {
        // Вместо msg![env; super init] используем msg_super!
        let this: id = msg_super![env; this init];

        if url != nil {
            retain(env, url);
        }
        env.objc.borrow_mut::<UIDocumentHostObject>(this).file_url = url;
        this
    }

    - (())dealloc {
        let (url, undo_manager) = {
            let host = env.objc.borrow::<UIDocumentHostObject>(this);
            (host.file_url, host.undo_manager)
        };
        if url != nil {
            release(env, url);
        }
        if undo_manager != nil {
            release(env, undo_manager);
        }
        // Здесь тоже меняем на msg_super!
        msg_super![env; this dealloc]
    }

    // MARK: Properties

    - (id)fileURL {
        env.objc.borrow::<UIDocumentHostObject>(this).file_url
    }

    // NSFilePresenter
    - (id)presentedItemURL {
        env.objc.borrow::<UIDocumentHostObject>(this).file_url
    }

    // Name without the extension.
    - (id)localizedName {
        let url = env.objc.borrow::<UIDocumentHostObject>(this).file_url;
        if url == nil {
            return nil;
        }
        let last: id = msg![env; url lastPathComponent];
        if last == nil {
            return nil;
        }
        msg![env; last stringByDeletingPathExtension]
    }

    // The file's UTI and modification date are not tracked.
    - (id)fileType {
        nil
    }

    - (id)fileModificationDate {
        nil
    }

    - (id)undoManager {
        env.objc.borrow::<UIDocumentHostObject>(this).undo_manager
    }

    - (())setUndoManager:(id)manager {
        if manager != nil {
            retain(env, manager);
        }
        let old = std::mem::replace(
            &mut env.objc.borrow_mut::<UIDocumentHostObject>(this).undo_manager,
            manager,
        );
        if old != nil {
            release(env, old);
        }
    }

    - (NSUInteger)documentState {
        let (open, saving_error, editing_disabled) = {
            let host = env.objc.borrow::<UIDocumentHostObject>(this);
            (host.open, host.saving_error, host.editing_disabled)
        };
        let mut state = UIDocumentStateNormal;
        if !open {
            state |= UIDocumentStateClosed;
        }
        if saving_error {
            state |= UIDocumentStateSavingError;
        }
        if editing_disabled {
            state |= UIDocumentStateEditingDisabled;
        }
        state
    }

    // MARK: Change tracking

    - (bool)hasUnsavedChanges {
        env.objc.borrow::<UIDocumentHostObject>(this).change_count != 0
    }

    - (())updateChangeCount:(NSInteger)change {
        let host = env.objc.borrow_mut::<UIDocumentHostObject>(this);
        match change {
            UIDocumentChangeDone | UIDocumentChangeRedone => host.change_count += 1,
            UIDocumentChangeUndone => host.change_count -= 1,
            UIDocumentChangeCleared => host.change_count = 0,
            _ => {}
        }
    }

    - (())disableEditing {
        env.objc.borrow_mut::<UIDocumentHostObject>(this).editing_disabled = true;
    }

    - (())enableEditing {
        env.objc.borrow_mut::<UIDocumentHostObject>(this).editing_disabled = false;
    }

    // MARK: Default override points

    // Subclasses normally override loadFromContents:ofType:error: instead.
    - (bool)readFromURL:(id)url error:(MutPtr<id>)_error {
        load_from_url(env, this, url)
    }

    // `contents` is NSData (NSFileWrapper packages are not supported).
    - (bool)writeContents:(id)contents
                    toURL:(id)url
         forSaveOperation:(NSInteger)_operation
      originalContentsURL:(id)_original_url
                    error:(MutPtr<id>)_error {
        if contents == nil || url == nil {
            return false;
        }
        let path: id = msg![env; url path];
        if path == nil || !responds_to(env, contents, "writeToFile:atomically:") {
            log!("UIDocument: cannot write contents {:?} to {:?}", contents, url);
            return false;
        }
        let ok: bool = msg![env; contents writeToFile:path atomically:true];
        ok
    }

    // MARK: Open / close / save

    - (())openWithCompletionHandler:(id)completionHandler {
        let url = env.objc.borrow::<UIDocumentHostObject>(this).file_url;
        let err = alloc_error_out(env);
        let ok: bool = if url == nil {
            false
        } else {
            msg![env; this readFromURL:url error:err]
        };
        env.mem.free(err.cast());
        if ok {
            let host = env.objc.borrow_mut::<UIDocumentHostObject>(this);
            host.open = true;
            host.change_count = 0;
        } else {
            log!("UIDocument openWithCompletionHandler: failed for {:?}", url);
        }
        call_bool_block(env, completionHandler, ok);
    }

    - (())closeWithCompletionHandler:(id)completionHandler {
        let (url, open, dirty) = {
            let host = env.objc.borrow::<UIDocumentHostObject>(this);
            (host.file_url, host.open, host.change_count != 0)
        };
        // Real UIKit saves outstanding changes when closing.
        if open && dirty && url != nil {
            let _: () = msg![env; this saveToURL:url
                                forSaveOperation:UIDocumentSaveForOverwriting
                               completionHandler:nil];
        }
        let ok = {
            let host = env.objc.borrow_mut::<UIDocumentHostObject>(this);
            host.open = false;
            !host.saving_error
        };
        call_bool_block(env, completionHandler, ok);
    }

    - (())saveToURL:(id)url
   forSaveOperation:(NSInteger)operation
  completionHandler:(id)completionHandler {
        let ok = save_to_url(env, this, url, operation);
        {
            let host = env.objc.borrow_mut::<UIDocumentHostObject>(this);
            host.saving_error = !ok;
            if ok {
                host.change_count = 0;
            }
        }
        call_bool_block(env, completionHandler, ok);
    }

    - (())autosaveWithCompletionHandler:(id)completionHandler {
        let (url, dirty) = {
            let host = env.objc.borrow::<UIDocumentHostObject>(this);
            (host.file_url, host.change_count != 0)
        };
        if dirty && url != nil {
            let _: () = msg![env; this saveToURL:url
                                forSaveOperation:UIDocumentSaveForOverwriting
                               completionHandler:completionHandler];
        } else {
            call_bool_block(env, completionHandler, true);
        }
    }

    - (())revertToContentsOfURL:(id)url completionHandler:(id)completionHandler {
        let err = alloc_error_out(env);
        let ok: bool = if url == nil {
            false
        } else {
            msg![env; this readFromURL:url error:err]
        };
        env.mem.free(err.cast());
        if ok {
            env.objc.borrow_mut::<UIDocumentHostObject>(this).change_count = 0;
        }
        call_bool_block(env, completionHandler, ok);
    }

    - (())performAsynchronousFileAccessUsingBlock:(id)block {
        call_void_block(env, block);
    }

    // MARK: Errors

    - (())handleError:(id)error userInteractionPermitted:(bool)_permitted {
        log!("UIDocument {:?} handleError: {:?}", this, error);
    }

    - (())finishedHandlingError:(id)_error recovered:(bool)_recovered {}

    - (())userInteractionNoLongerPermittedForError:(id)_error {}

    // MARK: Description

    - (id)description {
        let (state, dirty): (NSUInteger, bool) = {
            let host = env.objc.borrow::<UIDocumentHostObject>(this);
            let mut state = UIDocumentStateNormal;
            if !host.open {
                state |= UIDocumentStateClosed;
            }
            (state, host.change_count != 0)
        };
        let s = format!(
            "<UIDocument: {:?}; documentState = {:#x}; hasUnsavedChanges = {}>",
            this, state, dirty
        );
        let ns = from_rust_string(env, s);
        autorelease(env, ns)
    }

    @end
};
