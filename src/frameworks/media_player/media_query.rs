/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaQuery`, `MPMediaPredicate` and `MPMediaPropertyPredicate`.
//!
//! Apple docs: MPMediaQuery specifies a set of media items from the iPod
//! library by way of media property predicates. Class methods like
//! `playlistsQuery` and `songsQuery` return pre-configured query objects.
//!
//! Our implementation returns valid MPMediaQuery instances whose `items`
//! and `collections` properties return empty arrays (no media on the
//! emulated device). This allows apps that enumerate the user's music
//! library to proceed without crashing. Filter predicates and the grouping
//! type are stored (so they read back) but never applied.

use crate::frameworks::foundation::ns_string::from_rust_string;
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, Class, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;

/// `MPMediaGrouping`
pub type MPMediaGrouping = NSInteger;
pub const MPMediaGroupingTitle: MPMediaGrouping = 0;
pub const MPMediaGroupingAlbum: MPMediaGrouping = 1;
pub const MPMediaGroupingArtist: MPMediaGrouping = 2;
pub const MPMediaGroupingAlbumArtist: MPMediaGrouping = 3;
pub const MPMediaGroupingComposer: MPMediaGrouping = 4;
pub const MPMediaGroupingGenre: MPMediaGrouping = 5;
pub const MPMediaGroupingPlaylist: MPMediaGrouping = 6;
pub const MPMediaGroupingPodcastTitle: MPMediaGrouping = 7;

/// `MPMediaPredicateComparison`
pub type MPMediaPredicateComparison = NSInteger;
pub const MPMediaPredicateComparisonEqualTo: MPMediaPredicateComparison = 0;
pub const MPMediaPredicateComparisonContains: MPMediaPredicateComparison = 1;

#[derive(Default)]
struct MPMediaQueryHostObject {
    /// `NSMutableSet *` of predicates, retained. nil until one is added.
    filter_predicates: id,
    grouping_type: MPMediaGrouping,
}
impl HostObject for MPMediaQueryHostObject {}

#[derive(Default)]
struct MPMediaPropertyPredicateHostObject {
    /// Retained.
    value: id,
    /// `NSString *`, retained.
    property: id,
    comparison_type: MPMediaPredicateComparison,
}
impl HostObject for MPMediaPropertyPredicateHostObject {}

/// Allocates an autoreleased query of class `cls` with the given default
/// grouping.
fn new_query(env: &mut Environment, cls: Class, grouping: MPMediaGrouping) -> id {
    let obj: id = msg![env; cls alloc];
    let obj: id = msg![env; obj init];
    env.objc
        .borrow_mut::<MPMediaQueryHostObject>(obj)
        .grouping_type = grouping;
    autorelease(env, obj)
}

/// Allocates an autoreleased property predicate.
fn new_property_predicate(
    env: &mut Environment,
    cls: Class,
    value: id,
    property: id,
    comparison: MPMediaPredicateComparison,
) -> id {
    let obj: id = msg![env; cls alloc];
    retain(env, value);
    retain(env, property);
    {
        let host = env.objc.borrow_mut::<MPMediaPropertyPredicateHostObject>(obj);
        host.value = value;
        host.property = property;
        host.comparison_type = comparison;
    }
    autorelease(env, obj)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// MARK: - MPMediaQuery

@implementation MPMediaQuery: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    // Default grouping is MPMediaGroupingTitle (0).
    let host = Box::<MPMediaQueryHostObject>::default();
    env.objc.alloc_object(this, host, &mut env.mem)
}

// Each factory pre-sets the grouping type, as in UIKit.
+ (id)query {
    new_query(env, this, MPMediaGroupingTitle)
}
+ (id)songsQuery {
    new_query(env, this, MPMediaGroupingTitle)
}
+ (id)albumsQuery {
    new_query(env, this, MPMediaGroupingAlbum)
}
+ (id)artistsQuery {
    new_query(env, this, MPMediaGroupingArtist)
}
+ (id)playlistsQuery {
    new_query(env, this, MPMediaGroupingPlaylist)
}
+ (id)podcastsQuery {
    new_query(env, this, MPMediaGroupingPodcastTitle)
}
+ (id)audiobooksQuery {
    new_query(env, this, MPMediaGroupingTitle)
}
+ (id)compilationsQuery {
    new_query(env, this, MPMediaGroupingAlbum)
}
+ (id)composersQuery {
    new_query(env, this, MPMediaGroupingComposer)
}
+ (id)genresQuery {
    new_query(env, this, MPMediaGroupingGenre)
}

- (id)init {
    this
}

- (id)initWithFilterPredicates:(id)predicates {
    // `copy` of nil is nil. mutableCopy returns +1, which the host object owns.
    let set: id = msg![env; predicates mutableCopy];
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).filter_predicates,
        set,
    );
    release(env, old);
    this
}

- (())dealloc {
    let set = std::mem::take(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).filter_predicates,
    );
    release(env, set);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: NSCopying

- (id)copyWithZone:(NSZonePtr)_zone {
    let cls: Class = msg![env; this class];
    let new: id = msg![env; cls alloc];
    let new: id = msg![env; new init];
    let (set, grouping) = {
        let host = env.objc.borrow::<MPMediaQueryHostObject>(this);
        (host.filter_predicates, host.grouping_type)
    };
    // The copy gets its own set, so adding to one query can't change the other.
    let set_copy: id = msg![env; set mutableCopy];
    let old = {
        let host = env.objc.borrow_mut::<MPMediaQueryHostObject>(new);
        host.grouping_type = grouping;
        std::mem::replace(&mut host.filter_predicates, set_copy)
    };
    release(env, old);
    new
}

// MARK: Filter predicates

- (id)filterPredicates { // NSSet*, nil if none were added
    env.objc.borrow::<MPMediaQueryHostObject>(this).filter_predicates
}

- (())setFilterPredicates:(id)predicates {
    let set: id = msg![env; predicates mutableCopy];
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaQueryHostObject>(this).filter_predicates,
        set,
    );
    release(env, old);
}

- (())addFilterPredicate:(id)predicate {
    if predicate == nil {
        return;
    }
    let mut set = env.objc.borrow::<MPMediaQueryHostObject>(this).filter_predicates;
    if set == nil {
        // `new` gives +1, which the host object keeps.
        set = msg_class![env; NSMutableSet new];
        env.objc
            .borrow_mut::<MPMediaQueryHostObject>(this)
            .filter_predicates = set;
    }
    let _: () = msg![env; set addObject:predicate];
}

- (())removeFilterPredicate:(id)predicate {
    let set = env.objc.borrow::<MPMediaQueryHostObject>(this).filter_predicates;
    if set == nil || predicate == nil {
        return;
    }
    let _: () = msg![env; set removeObject:predicate];
}

// MARK: Grouping

- (NSInteger)groupingType {
    env.objc.borrow::<MPMediaQueryHostObject>(this).grouping_type
}

- (())setGroupingType:(NSInteger)grouping_type {
    env.objc
        .borrow_mut::<MPMediaQueryHostObject>(this)
        .grouping_type = grouping_type;
}

// MARK: Results (always empty: there is no media library)

- (id)items {
    // Empty NSArray: no media items available.
    msg_class![env; NSArray array]
}

- (id)collections {
    // Empty NSArray: no collections (playlists/albums) available.
    msg_class![env; NSArray array]
}

- (id)itemSections {
    // Sections of nothing.
    nil
}

- (id)collectionSections {
    nil
}

// MARK: Description

- (id)description {
    let (set, grouping) = {
        let host = env.objc.borrow::<MPMediaQueryHostObject>(this);
        (host.filter_predicates, host.grouping_type)
    };
    let predicate_count: NSUInteger = if set == nil { 0 } else { msg![env; set count] };
    let s = format!(
        "<MPMediaQuery: {:?}; groupingType = {}; filterPredicates = {}>",
        this, grouping, predicate_count
    );
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

@end

// MARK: - MPMediaPredicate / MPMediaPropertyPredicate

// Abstract base; only exists so MPMediaPropertyPredicate has the right
// superclass (`isKindOfClass:` checks).
@implementation MPMediaPredicate: NSObject
@end

@implementation MPMediaPropertyPredicate: MPMediaPredicate

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::<MPMediaPropertyPredicateHostObject>::default();
    env.objc.alloc_object(this, host, &mut env.mem)
}

+ (id)predicateWithValue:(id)value forProperty:(id)property {
    new_property_predicate(env, this, value, property, MPMediaPredicateComparisonEqualTo)
}

+ (id)predicateWithValue:(id)value
             forProperty:(id)property
          comparisonType:(NSInteger)comparison_type {
    new_property_predicate(env, this, value, property, comparison_type)
}

- (id)value {
    env.objc.borrow::<MPMediaPropertyPredicateHostObject>(this).value
}

- (id)property {
    env.objc.borrow::<MPMediaPropertyPredicateHostObject>(this).property
}

- (NSInteger)comparisonType {
    env.objc.borrow::<MPMediaPropertyPredicateHostObject>(this).comparison_type
}

- (())dealloc {
    let (value, property) = {
        let host = env.objc.borrow::<MPMediaPropertyPredicateHostObject>(this);
        (host.value, host.property)
    };
    release(env, value);
    release(env, property);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};
