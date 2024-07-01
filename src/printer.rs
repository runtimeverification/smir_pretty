use std::fs::File;
use std::io;
use std::iter::Iterator;
use std::vec::Vec;
use std::str;
extern crate rustc_data_structures;
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_monomorphize;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_smir;
extern crate stable_mir;
// use rustc_hir::{def::DefKind, definitions::DefPath};
use tracing::{debug, debug_span, trace};
use rustc_data_structures::fx::FxHashSet;
use rustc_data_structures::fingerprint::Fingerprint;
use rustc_data_structures::stable_hasher::{HashStable, StableHasher};
use rustc_hir::lang_items::LangItem;
use rustc_middle::traits::{ImplSource, ImplSourceUserDefinedData};
use rustc_middle::ty::adjustment::CustomCoerceUnsized;
use rustc_middle::ty::{TyCtxt, Ty, TyKind, EarlyBinder, FnSig, GenericArgs, TypeFoldable, ParamEnv, VtblEntry, TraitRef}; // Binder Generics, GenericPredicates
use rustc_session::config::{OutFileName, OutputType};
use rustc_span::{def_id::DefId, symbol}; // symbol::sym::test;
use rustc_smir::rustc_internal;
use stable_mir::CrateItem;
use stable_mir::{CrateDef,ItemKind,to_json,mir::Body,ty::ForeignItemKind, mir::mono::{Instance, InstanceKind, MonoItem, StaticDef}, global_allocs, scc_accessor}; // Symbol
use stable_mir::mir::{visit::Location, MirVisitor, Rvalue, CastKind, PointerCoercion, Terminator, TerminatorKind, Constant};
use stable_mir::ty::{RigidTy, ClosureKind, ConstantKind, Allocation, Ty as TyStable, TyKind as TyKindStable};
use stable_mir::mir::alloc::{AllocId, GlobalAlloc};
use stable_mir::Symbol;
use tracing::enabled;
use serde::Serialize;

use crate::coercion::CoercionBase;

// TODO: consider using underlying structs struct GenericData<'a>(Vec<(&'a Generics,GenericPredicates<'a>)>);
#[derive(Serialize)]
struct GenericData(Vec<(String,String)>);
#[derive(Serialize)]
struct ItemDetails {
    internal_kind: String,
    path: String,
    internal_ty: String,
    generic_data: GenericData,
}
#[derive(Serialize)]
struct BodyDetails {
    pp: String,
}

#[derive(Serialize)]
struct MirBody(Body, Option<BodyDetails>);
#[derive(Serialize)]
enum MonoItemKind {
    MonoItemFn,
    MonoItemStatic,
    MonoItemGlobalAsm
}
#[derive(Serialize)]
struct Item {
    name: String,
    mono_item_kind: MonoItemKind,
    id: Option<stable_mir::DefId>,
    instance_kind: Option<InstanceKind>,
    item_kind: Option<ItemKind>,
    body: Option<MirBody>,
    promoted: Option<Vec<MirBody>>,
    details: Option<ItemDetails>
}
#[derive(Serialize)]
struct ForeignItem {
    name: String,
    kind: ForeignItemKind,
}
#[derive(Serialize)]
struct ForeignModule {
    name: String,
    items: Vec<ForeignItem>,
}
#[derive(Serialize)]
struct InstanceData {
    internal_id: String,
    instance: Instance,
}
#[derive(Serialize)]
struct CrateData {
    name: String,
    items: Vec<Item>,
}

fn generic_data(tcx: TyCtxt<'_>, id: DefId) -> GenericData {
     let mut v = Vec::new();
     let mut next_id = Some(id);
     while let Some(_curr_id) = next_id {
        let params = tcx.generics_of(id);
        let preds  = tcx.predicates_of(id);
        if params.parent != preds.parent { panic!("Generics and predicates parent ids are distinct"); }
        v.push((format!("{:#?}", params), format!("{:#?}", preds)));
        next_id = params.parent;
     }
     v.reverse();
     return GenericData(v);
}

fn get_body_details(body: &Body, name: Option<&String>) -> Option<BodyDetails> {
  if enabled!(tracing::Level::DEBUG) {
    let mut v = Vec::new();
    let name = if let Some(name) = name { name } else { "<promoted>" };
    let _ = body.dump(&mut v, name);
    Some(BodyDetails {
      pp: str::from_utf8(&v).unwrap().into(),
    })
  } else {
    None
  }
}

// unwrap early binder in a default manner; panic on error
fn default_unwrap_early_binder<'tcx, T>(tcx: TyCtxt<'tcx>, id: DefId, v: EarlyBinder<T>) -> T
  where T: TypeFoldable<TyCtxt<'tcx>>
{
  let v_copy = v.clone();
  match tcx.try_instantiate_and_normalize_erasing_regions(GenericArgs::identity_for_item(tcx, id), tcx.param_env(id), v) {
      Ok(res) => return res,
      Err(err) => { println!("{:?}", err); v_copy.skip_binder() }
  }
}

fn print_type<'tcx>(tcx: TyCtxt<'tcx>, id: DefId, ty: EarlyBinder<Ty<'tcx>>) -> String {
  // lookup type kind in order to perform case analysis
  let kind: &TyKind = ty.skip_binder().kind();
  if let TyKind::FnDef(fun_id, args) = kind {
    // since FnDef doesn't contain signature, lookup actual function type
    // via getting fn signature with parameters and resolving those parameters
    let sig0 = tcx.fn_sig(fun_id);
    let sig1 = match tcx.try_instantiate_and_normalize_erasing_regions(args, tcx.param_env(fun_id), sig0) {
      Ok(res) => res,
      Err(err) => { println!("{:?}", err); sig0.skip_binder() }
    };
    let sig2: FnSig<'_> = tcx.instantiate_bound_regions_with_erased(sig1);
    format!("\nTyKind(FnDef): {:#?}", sig2)
  } else {
    let kind = default_unwrap_early_binder(tcx, id, ty);
    format!("\nTyKind: {:#?}", kind)
  }
}

fn get_item_details(tcx: TyCtxt<'_>, id: DefId) -> Option<ItemDetails> {
  if enabled!(tracing::Level::DEBUG) {
    Some(ItemDetails {
      internal_kind: format!("{:#?}", tcx.def_kind(id)),
      path: tcx.def_path_str(id),  // NOTE: underlying data from tcx.def_path(id);
      internal_ty: print_type(tcx, id, tcx.type_of(id)),
      generic_data: generic_data(tcx, id),
      // TODO: let layout = tcx.layout_of(id);
    })
  } else {
    None
  }
}

// Possible input: sym::test
pub fn has_attr(tcx: TyCtxt<'_>, item: &stable_mir::CrateItem, attr: symbol::Symbol) -> bool {
   tcx.has_attr(rustc_internal::internal(tcx,item), attr)
}

fn mk_mir_body(body: Body, name: Option<&String>) -> MirBody {
  let details = get_body_details(&body, name);
  MirBody(body, details)
}

// TODO: Should we filter any incoming items?
//       Example: .filter(|item| has_attr(item, sym::test) or matches!(item.kind, ItemKind::Const | ItemKind::Static | ItemKind::Fn))
fn emit_smir_internal(tcx: TyCtxt<'_>, writer: &mut dyn io::Write) {
  let local_crate = stable_mir::local_crate();
  // From kani compiler_interface.rs
  // From kani reachability.rs
  let maybe_main_instance = stable_mir::entry_fn().map(|main_fn| Instance::try_from(main_fn).ok()).flatten();
  // let main_instance = match maybe_main_instance {
  //   Some(instance) => instance,
  //   None => return,
  // };
  let initial_mono_items: Vec<MonoItem> = filter_crate_items(tcx, |_, instance| {
    let def_id = rustc_internal::internal(tcx, instance.def.def_id());
    Some(instance) == maybe_main_instance || tcx.is_reachable_non_generic(def_id)
  })
    .into_iter()
    .map(MonoItem::Fn)
    .collect();
  let all_mono_items = collect_all_mono_items(tcx, &initial_mono_items);
  let items: Vec<Item> = all_mono_items.iter().map(|item| {
      match item {
        MonoItem::Fn(item) => {
          // let item = rustc_internal::stable(instance);
          let body = item.body();
          let id = item.def.def_id();
          let name = item.name();
          let internal_id = rustc_internal::internal(tcx,id);
          Item {
            name: name.clone(),
            mono_item_kind: MonoItemKind::MonoItemFn,
            id:   Some(id),
            instance_kind: Some(item.kind),
            item_kind: CrateItem::try_from(*item).map_or(None, |item| Some(item.kind())),
            body: body.map_or(None, |body| { Some(mk_mir_body(body, Some(&name))) }),
            promoted: if item.has_body() { Some(tcx.promoted_mir(internal_id).into_iter().map(|body| mk_mir_body(rustc_internal::stable(body), None)).collect()) } else { None },
            details: get_item_details(tcx, internal_id),
          }
        },
        MonoItem::Static(static_def) => {
          let internal_id = rustc_internal::internal(tcx,static_def.def_id());
          Item {
            name: static_def.name(),
            mono_item_kind: MonoItemKind::MonoItemStatic,
            id:   Some(static_def.def_id()),
            instance_kind: None,
            item_kind: None,
            body: None,
            promoted: None,
            details: get_item_details(tcx, internal_id),
          }
        },
        MonoItem::GlobalAsm(asm) => {
          Item {
            name: format!("{:#?}", asm),
            mono_item_kind: MonoItemKind::MonoItemGlobalAsm,
            id:   None,
            instance_kind: None,
            item_kind: None,
            body: None,
            promoted: None,
            details: None,
          }
        }
      }
    }).collect();
  let crate_data = CrateData { name: local_crate.name,
                               items: items,
                             };
  writer.write_all("{\"crates\":".as_bytes()).unwrap();
  scc_accessor(|| {
    writer.write_all(to_json(crate_data).expect("serde_json failed").as_bytes()).expect("internal error: writing SMIR JSON failed");
    writer.write_all(",\"gallocs\":".as_bytes()).unwrap();
    writer.write_all(to_json(global_allocs()).expect("global_allocs failed").as_bytes()).expect("internal error: writing global_allocs JSON failed");
  });
  writer.write_all("}".as_bytes()).unwrap();
}

pub fn emit_smir(tcx: TyCtxt<'_>) {
  match tcx.output_filenames(()).path(OutputType::Mir) {
    OutFileName::Stdout => {
        let mut f = io::stdout();
        emit_smir_internal(tcx, &mut f);
    }
    OutFileName::Real(path) => {
        let mut f = io::BufWriter::new(File::create(&path.with_extension("smir.json")).expect("Failed to create SMIR output file"));
        emit_smir_internal(tcx, &mut f);
    }
  }
}

fn collect_all_mono_items(tcx: TyCtxt, initial_mono_items: &[MonoItem]) -> Vec<MonoItem> {
  let mut collector = MonoItemsCollector::new(tcx);
  for item in initial_mono_items {
    collector.collect(item.clone());
  }

  tcx.dcx().abort_if_errors();
  // Sort the result so code generation follows deterministic order.
  // This helps us to debug the code, but it also provides the user a good experience since the
  // order of the errors and warnings is stable.
  let mut sorted_items: Vec<_> = collector.collected.into_iter().collect();
  sorted_items.sort_by_cached_key(|item| to_fingerprint(tcx, item));
  sorted_items
}

struct MonoItemsCollector<'tcx> {
  /// The compiler context.
  tcx: TyCtxt<'tcx>,
  /// Set of collected items used to avoid entering recursion loops.
  collected: FxHashSet<MonoItem>,
  /// Items enqueued for visiting.
  queue: Vec<MonoItem>,
}

impl<'tcx> MonoItemsCollector<'tcx> {
  pub fn new(tcx: TyCtxt<'tcx>) -> Self {
    MonoItemsCollector {
      tcx,
      collected: FxHashSet::default(),
      queue: vec![],
    }
  }

  pub fn collect(&mut self, root: MonoItem) {
    self.queue.push(root);
    self.reachable_items();
  }

  fn reachable_items(&mut self) {
    while let Some(to_visit) = self.queue.pop() {
      if !self.collected.contains(&to_visit) {
        self.collected.insert(to_visit.clone());
        let next_items = match &to_visit {
          MonoItem::Fn(instance) => self.visit_fn(*instance),
          MonoItem::Static(static_def) => self.visit_static(*static_def),
          MonoItem::GlobalAsm(_) => {
            vec![]
          },
        };

        self.queue
          .extend(next_items.into_iter().filter(|item| !self.collected.contains(item)));
      }
    }
  }

  fn visit_fn(&mut self, instance: Instance) -> Vec<MonoItem> {
    let _guard = debug_span!("visit_fn", function=?instance).entered();
    if let Some(body) = instance.body() {
      let mut collector = MonoItemsFnCollector { tcx: self.tcx, collected: FxHashSet::default(), body: &body };
      collector.visit_body(&body);
      collector.collected.into_iter().collect()
    } else {
      // println!("{instance:#?}");
      vec![]
    }
  }

  /// Visit a static object and collect drop / initialization functions.
  fn visit_static(&mut self, def: StaticDef) -> Vec<MonoItem> {
    let _guard = debug_span!("visit_static", ?def).entered();
    let mut next_items = vec![];
    
    // Collect drop function.
    let static_ty = def.ty();
    let instance = Instance::resolve_drop_in_place(static_ty);
    next_items.push(instance.into());
    
    // Collect initialization.
    let alloc = def.eval_initializer().unwrap();
    for (_, prov) in alloc.provenance.ptrs {
        next_items.extend(collect_alloc_items(prov.0).into_iter());
    }
    
    next_items
  } 

  /// Visit global assembly and collect its item.
  fn visit_asm(&mut self, item: &MonoItem) {
    debug!(?item, "visit_asm");
  }
}

struct MonoItemsFnCollector<'a, 'tcx> {
  tcx: TyCtxt<'tcx>,
  collected: FxHashSet<MonoItem>,
  body: &'a Body,
}

impl<'a, 'tcx> MonoItemsFnCollector<'a, 'tcx> {
  /// Collect the implementation of all trait methods and its supertrait methods for the given
  /// concrete type.
  fn collect_vtable_methods(&mut self, concrete_ty: TyStable, trait_ty: TyStable) {
      trace!(?concrete_ty, ?trait_ty, "collect_vtable_methods");
      let concrete_kind = concrete_ty.kind();
      let trait_kind = trait_ty.kind();

      assert!(!concrete_kind.is_trait(), "expected a concrete type, but found `{concrete_ty:?}`");
      assert!(trait_kind.is_trait(), "expected a trait `{trait_ty:?}`");
      if let Some(principal) = trait_kind.trait_principal() {
          // A trait object type can have multiple trait bounds but up to one non-auto-trait
          // bound. This non-auto-trait, named principal, is the only one that can have methods.
          // https://doc.rust-lang.org/reference/special-types-and-traits.html#auto-traits
          let poly_trait_ref = principal.with_self_ty(concrete_ty);

          // Walk all methods of the trait, including those of its supertraits
          let entries =
              self.tcx.vtable_entries(rustc_internal::internal(self.tcx, &poly_trait_ref));
          let methods = entries.iter().filter_map(|entry| match entry {
              VtblEntry::MetadataAlign
              | VtblEntry::MetadataDropInPlace
              | VtblEntry::MetadataSize
              | VtblEntry::Vacant => None,
              VtblEntry::TraitVPtr(_) => {
                  // all super trait items already covered, so skip them.
                  None
              }
              VtblEntry::Method(instance) => {
                  let instance = rustc_internal::stable(instance);
                  should_codegen_locally(&instance).then_some(MonoItem::Fn(instance))
              }
          });
          trace!(methods=?methods.clone().collect::<Vec<_>>(), "collect_vtable_methods");
          self.collected.extend(methods);
      }

      // Add the destructor for the concrete type.
      let instance = Instance::resolve_drop_in_place(concrete_ty);
      self.collect_instance(instance, false);
  }

  /// Collect an instance depending on how it is used (invoked directly or via fn_ptr).
  fn collect_instance(&mut self, instance: Instance, is_direct_call: bool) {
      let should_collect = match instance.kind {
          InstanceKind::Virtual { .. } | InstanceKind::Intrinsic => { // TODO: Outdated
              // Instance definition has no body.
              assert!(is_direct_call, "Expected direct call {instance:?}");
              false
          }
          InstanceKind::Shim | InstanceKind::Item => true,
      };
      if should_collect && should_codegen_locally(&instance) {
          trace!(?instance, "collect_instance");
          self.collected.insert(instance.into());
      }
  }

  /// Collect constant values represented by static variables.
  fn collect_allocation(&mut self, alloc: &Allocation) {
      debug!(?alloc, "collect_allocation");
      for (_, id) in &alloc.provenance.ptrs {
          self.collected.extend(collect_alloc_items(id.0).into_iter())
      }
  }
}

/// Visit every instruction in a function and collect the following:
/// 1. Every function / method / closures that may be directly invoked.
/// 2. Every function / method / closures that may have their address taken.
/// 3. Every method that compose the impl of a trait for a given type when there's a conversion
///    from the type to the trait.
///    - I.e.: If we visit the following code:
///      ```
///      let var = MyType::new();
///      let ptr : &dyn MyTrait = &var;
///      ```
///      We collect the entire implementation of `MyTrait` for `MyType`.
/// 4. Every Static variable that is referenced in the function or constant used in the function.
/// 5. Drop glue.
/// 6. Static Initialization
///
/// Remark: This code has been mostly taken from `rustc_monomorphize::collector::MirNeighborCollector`.
impl<'a, 'tcx> MirVisitor for MonoItemsFnCollector<'a, 'tcx> {
    /// Collect the following:
    /// - Trait implementations when casting from concrete to dyn Trait.
    /// - Functions / Closures that have their address taken.
    /// - Thread Local.
    fn visit_rvalue(&mut self, rvalue: &Rvalue, location: Location) {
        trace!(rvalue=?*rvalue, "visit_rvalue");

        match *rvalue {
            Rvalue::Cast(
                CastKind::PointerCoercion(PointerCoercion::Unsize),
                ref operand,
                target,
            ) => {
                // Check if the conversion include casting a concrete type to a trait type.
                // If so, collect items from the impl `Trait for Concrete {}`.
                let target_ty = target;
                let source_ty = operand.ty(self.body.locals()).unwrap();
                let (src_ty, dst_ty) = extract_unsize_coercion(self.tcx, source_ty, target_ty);
                if !src_ty.kind().is_trait() && dst_ty.kind().is_trait() {
                    debug!(?src_ty, ?dst_ty, "collect_vtable_methods");
                    self.collect_vtable_methods(src_ty, dst_ty);
                }
            }
            Rvalue::Cast(
                CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer),
                ref operand,
                _,
            ) => {
                let fn_kind = operand.ty(self.body.locals()).unwrap().kind();
                if let RigidTy::FnDef(fn_def, args) = fn_kind.rigid().unwrap() {
                    let instance = Instance::resolve_for_fn_ptr(*fn_def, args).unwrap();
                    self.collect_instance(instance, false);
                } else {
                    unreachable!("Expected FnDef type, but got: {:?}", fn_kind);
                }
            }
            Rvalue::Cast(
                CastKind::PointerCoercion(PointerCoercion::ClosureFnPointer(_)),
                ref operand,
                _,
            ) => {
                let source_ty = operand.ty(self.body.locals()).unwrap();
                match source_ty.kind().rigid().unwrap() {
                    RigidTy::Closure(def_id, args) => {
                        let instance =
                            Instance::resolve_closure(*def_id, args, ClosureKind::FnOnce)
                                .expect("failed to normalize and resolve closure during codegen");
                        self.collect_instance(instance, false);
                    }
                    _ => unreachable!("Unexpected type: {:?}", source_ty),
                }
            }
            Rvalue::ThreadLocalRef(item) => {
                trace!(?item, "visit_rvalue thread_local");
                self.collected.insert(MonoItem::Static(StaticDef::try_from(item).unwrap()));
            }
            _ => { /* not interesting */ }
        }

        self.super_rvalue(rvalue, location);
    }

    /// Collect constants that are represented as static variables.
    fn visit_constant(&mut self, constant: &Constant, location: Location) {
        debug!(?constant, ?location, literal=?constant.literal, "visit_constant");
        let allocation = match constant.literal.kind() {
            ConstantKind::Allocated(allocation) => allocation,
            ConstantKind::Unevaluated(_) => {
                unreachable!("Instance with polymorphic constant: `{constant:?}`")
            }
            ConstantKind::Param(_) => unreachable!("Unexpected parameter constant: {constant:?}"),
            ConstantKind::ZeroSized => {
                // Nothing to do here.
                return;
            }
        };
        self.collect_allocation(&allocation);
    }

    /// Collect function calls.
    fn visit_terminator(&mut self, terminator: &Terminator, location: Location) {
        trace!(?terminator, ?location, "visit_terminator");

        match terminator.kind {
            TerminatorKind::Call { ref func, .. } => {
                let fn_ty = func.ty(self.body.locals()).unwrap();
                if let TyKindStable::RigidTy(RigidTy::FnDef(fn_def, args)) = fn_ty.kind() {
                    let instance = Instance::resolve(fn_def, &args).unwrap();
                    self.collect_instance(instance, true);
                } else {
                    assert!(
                        matches!(fn_ty.kind().rigid(), Some(RigidTy::FnPtr(..))),
                        "Unexpected type: {fn_ty:?}"
                    );
                }
            }
            TerminatorKind::Drop { ref place, .. } => {
                let place_ty = place.ty(self.body.locals()).unwrap();
                let instance = Instance::resolve_drop_in_place(place_ty);
                self.collect_instance(instance, true);
            }
            TerminatorKind::InlineAsm { .. } => {
                // We don't support inline assembly. This shall be replaced by an unsupported
                // construct during codegen.
            }
            TerminatorKind::Abort { .. } | TerminatorKind::Assert { .. } => {
                // We generate code for this without invoking any lang item.
            }
            TerminatorKind::Goto { .. }
            | TerminatorKind::SwitchInt { .. }
            | TerminatorKind::Resume
            | TerminatorKind::Return
            | TerminatorKind::Unreachable => {}
        }

        self.super_terminator(terminator, location);
    }
}

fn extract_unsize_coercion(tcx: TyCtxt, orig_ty: TyStable, dst_trait: TyStable) -> (TyStable, TyStable) {
  let CoercionBase { src_ty, dst_ty } = extract_unsize_casting(
      tcx,
      rustc_internal::internal(tcx, orig_ty),
      rustc_internal::internal(tcx, dst_trait),
  );
  (rustc_internal::stable(src_ty), rustc_internal::stable(dst_ty))
}

/// Return whether we should include the item into codegen.
fn should_codegen_locally(instance: &Instance) -> bool {
  !instance.is_foreign_item()
}

/// Convert a `MonoItem` into a stable `Fingerprint` which can be used as a stable hash across
/// compilation sessions. This allow us to provide a stable deterministic order to codegen.
fn to_fingerprint(tcx: TyCtxt, item: &MonoItem) -> Fingerprint {
  tcx.with_stable_hashing_context(|mut hcx| {
      let mut hasher = StableHasher::new();
      rustc_internal::internal(tcx, item).hash_stable(&mut hcx, &mut hasher);
      hasher.finish()
  })
}

fn collect_alloc_items(alloc_id: AllocId) -> Vec<MonoItem> {
  trace!(?alloc_id, "collect_alloc_items");
  let mut items = vec![];
  match GlobalAlloc::from(alloc_id) {
      GlobalAlloc::Static(def) => {
          // This differ from rustc's collector since rustc does not include static from
          // upstream crates.
          let instance = Instance::try_from(CrateItem::from(def)).unwrap();
          should_codegen_locally(&instance).then(|| items.push(MonoItem::from(def)));
      }
      GlobalAlloc::Function(instance) => {
          should_codegen_locally(&instance).then(|| items.push(MonoItem::from(instance)));
      }
      GlobalAlloc::Memory(alloc) => {
          items.extend(
              alloc.provenance.ptrs.iter().flat_map(|(_, prov)| collect_alloc_items(prov.0)),
          );
      }
      vtable_alloc @ GlobalAlloc::VTable(..) => {
          let vtable_id = vtable_alloc.vtable_allocation().unwrap();
          items = collect_alloc_items(vtable_id);
      }
  };
  items
}

/// Collect all (top-level) items in the crate that matches the given predicate.
/// An item can only be a root if they are a non-generic function.
pub fn filter_crate_items<F>(tcx: TyCtxt, predicate: F) -> Vec<Instance>
where
    F: Fn(TyCtxt, Instance) -> bool,
{
    let crate_items = stable_mir::all_local_items();
    // Filter regular items.
    crate_items
        .iter()
        .filter_map(|item| {
            // Only collect monomorphic items.
            // TODO: Remove the def_kind check once https://github.com/rust-lang/rust/pull/119135 has been released.
            let def_id = rustc_internal::internal(tcx, item.def_id());
            (matches!(tcx.def_kind(def_id), rustc_hir::def::DefKind::Ctor(..))
                || matches!(item.kind(), ItemKind::Fn))
            .then(|| {
                Instance::try_from(*item)
                    .ok()
                    .and_then(|instance| predicate(tcx, instance).then_some(instance))
            })
            .flatten()
        })
        .collect::<Vec<_>>()
}

pub fn extract_unsize_casting<'tcx>(
    tcx: TyCtxt<'tcx>,
    src_ty: Ty<'tcx>,
    dst_ty: Ty<'tcx>,
) -> CoercionBase<'tcx> {
    trace!(?src_ty, ?dst_ty, "extract_unsize_casting");
    // Iterate over the pointer structure to find the builtin pointer that will store the metadata.
    let coerce_info = CoerceUnsizedIterator::new(
        tcx,
        rustc_internal::stable(src_ty),
        rustc_internal::stable(dst_ty),
    )
    .last()
    .unwrap();
    // Extract the pointee type that is being coerced.
    let src_pointee_ty = extract_pointee(tcx, coerce_info.src_ty).expect(&format!(
        "Expected source to be a pointer. Found {:?} instead",
        coerce_info.src_ty
    ));
    let dst_pointee_ty = extract_pointee(tcx, coerce_info.dst_ty).expect(&format!(
        "Expected destination to be a pointer. Found {:?} instead",
        coerce_info.dst_ty
    ));
    // Find the tail of the coercion that determines the type of metadata to be stored.
    let (src_base_ty, dst_base_ty) = tcx.struct_lockstep_tails_erasing_lifetimes(
        src_pointee_ty,
        dst_pointee_ty,
        ParamEnv::reveal_all(),
    );
    trace!(?src_base_ty, ?dst_base_ty, "extract_unsize_casting result");
    assert!(
        dst_base_ty.is_trait() || dst_base_ty.is_slice(),
        "Expected trait or slice as destination of unsized cast, but found {dst_base_ty:?}"
    );
    CoercionBase { src_ty: src_base_ty, dst_ty: dst_base_ty }
}

/// This structure represents the base of a coercion.
///
/// This base is used to determine the information that will be stored in the metadata.
/// E.g.: In order to convert an `Rc<String>` into an `Rc<dyn Debug>`, we need to generate a
/// vtable that represents the `impl Debug for String`. So this type will carry the `String` type
/// as the `src_ty` and the `dyn Debug` trait as `dst_ty`.

/// Iterates over the coercion path of a structure that implements `CoerceUnsized<T>` trait.
/// The `CoerceUnsized<T>` trait indicates that this is a pointer or a wrapper for one, where
/// unsizing can be performed on the pointee. More details:
/// <https://doc.rust-lang.org/std/ops/trait.CoerceUnsized.html>
///
/// Given an unsized coercion between `impl CoerceUnsized<T>` to `impl CoerceUnsized<U>` where
/// `T` is sized and `U` is unsized, this iterator will walk over the fields that lead to a
/// pointer to `T`, which shall be converted from a thin pointer to a fat pointer.
///
/// Each iteration will also include an optional name of the field that differs from the current
/// pair of types.
///
/// The first element of the iteration will always be the starting types.
/// The last element of the iteration will always be pointers to `T` and `U`.
/// After unsized element has been found, the iterator will return `None`.
pub struct CoerceUnsizedIterator<'tcx> {
    tcx: TyCtxt<'tcx>,
    src_ty: Option<TyStable>,
    dst_ty: Option<TyStable>,
}

/// Represent the information about a coercion.
#[derive(Debug, Clone, PartialEq)]
pub struct CoerceUnsizedInfo {
    /// The name of the field from the current types that differs between each other.
    pub field: Option<Symbol>,
    /// The type being coerced.
    pub src_ty: TyStable,
    /// The type that is the result of the coercion.
    pub dst_ty: TyStable,
}

impl<'tcx> CoerceUnsizedIterator<'tcx> {
    pub fn new(
        tcx: TyCtxt<'tcx>,
        src_ty: TyStable,
        dst_ty: TyStable,
    ) -> CoerceUnsizedIterator<'tcx> {
        CoerceUnsizedIterator { tcx, src_ty: Some(src_ty), dst_ty: Some(dst_ty) }
    }
}

/// Iterate over the coercion path. At each iteration, it returns the name of the field that must
/// be coerced, as well as the current source and the destination.
/// E.g.: The first iteration of casting `NonNull<String>` -> `NonNull<&dyn Debug>` will return
/// ```rust,ignore
/// CoerceUnsizedInfo {
///    field: Some("ptr"),
///    src_ty, // NonNull<String>
///    dst_ty  // NonNull<&dyn Debug>
/// }
/// ```
/// while the last iteration will return:
/// ```rust,ignore
/// CoerceUnsizedInfo {
///   field: None,
///   src_ty: Ty, // *const String
///   dst_ty: Ty, // *const &dyn Debug
/// }
/// ```
impl<'tcx> Iterator for CoerceUnsizedIterator<'tcx> {
    type Item = CoerceUnsizedInfo;

    fn next(&mut self) -> Option<Self::Item> {
        if self.src_ty.is_none() {
            assert_eq!(self.dst_ty, None, "Expected no dst type.");
            return None;
        }

        // Extract the pointee types from pointers (including smart pointers) that form the base of
        // the conversion.
        let src_ty = self.src_ty.take().unwrap();
        let dst_ty = self.dst_ty.take().unwrap();
        let field = match (src_ty.kind(), dst_ty.kind()) {
            (
                TyKindStable::RigidTy(RigidTy::Adt(src_def, ref src_args)),
                TyKindStable::RigidTy(RigidTy::Adt(dst_def, ref dst_args)),
            ) => {
                // Handle smart pointers by using CustomCoerceUnsized to find the field being
                // coerced.
                assert_eq!(src_def, dst_def);
                let src_fields = &src_def.variants_iter().next().unwrap().fields();
                let dst_fields = &dst_def.variants_iter().next().unwrap().fields();
                assert_eq!(src_fields.len(), dst_fields.len());

                let CustomCoerceUnsized::Struct(coerce_index) = custom_coerce_unsize_info(
                    self.tcx,
                    rustc_internal::internal(self.tcx, src_ty),
                    rustc_internal::internal(self.tcx, dst_ty),
                );
                let coerce_index = coerce_index.as_usize();
                assert!(coerce_index < src_fields.len());

                self.src_ty = Some(src_fields[coerce_index].ty_with_args(&src_args));
                self.dst_ty = Some(dst_fields[coerce_index].ty_with_args(&dst_args));
                Some(src_fields[coerce_index].name.clone())
            }
            _ => {
                // Base case is always a pointer (Box, raw_pointer or reference).
                assert!(
                    extract_pointee(self.tcx, src_ty).is_some(),
                    "Expected a pointer, but found {src_ty:?}"
                );
                None
            }
        };
        Some(CoerceUnsizedInfo { field, src_ty, dst_ty })
    }
}

/// Get information about an unsized coercion.
/// This code was extracted from `rustc_monomorphize` crate.
/// <https://github.com/rust-lang/rust/blob/4891d57f7aab37b5d6a84f2901c0bb8903111d53/compiler/rustc_monomorphize/src/lib.rs#L25-L46>
fn custom_coerce_unsize_info<'tcx>(
    tcx: TyCtxt<'tcx>,
    source_ty: Ty<'tcx>,
    target_ty: Ty<'tcx>,
) -> CustomCoerceUnsized {
    let def_id = tcx.require_lang_item(LangItem::CoerceUnsized, None);

    let trait_ref = TraitRef::new(tcx, def_id, tcx.mk_args_trait(source_ty, [target_ty.into()]));

    match tcx.codegen_select_candidate((ParamEnv::reveal_all(), trait_ref)) {
        Ok(ImplSource::UserDefined(ImplSourceUserDefinedData { impl_def_id, .. })) => {
            tcx.coerce_unsized_info(impl_def_id).unwrap().custom_kind.unwrap()
        }
        impl_source => {
            unreachable!("invalid `CoerceUnsized` impl_source: {:?}", impl_source);
        }
    }
}

/// Extract pointee type from builtin pointer types.
fn extract_pointee(tcx: TyCtxt<'_>, typ: TyStable) -> Option<Ty<'_>> {
    rustc_internal::internal(tcx, typ).builtin_deref(true)
}