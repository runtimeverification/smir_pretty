use std::{collections::HashMap,fs::File,io,iter::Iterator,vec::Vec,str,};
extern crate rustc_middle;
extern crate rustc_monomorphize;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_smir;
extern crate stable_mir;
// HACK: typically, we would source serde/serde_json separately from the compiler
//       However, due to issues matching crate versions when we have our own serde
//       in addition to the rustc serde, we force ourselves to use rustc serde
extern crate serde;
extern crate serde_json;
use rustc_middle as middle;
use rustc_middle::ty::{TyCtxt, Ty, TyKind, EarlyBinder, FnSig, GenericArgs, TypeFoldable, ParamEnv}; //, Binder, Generics, GenericPredicates
use rustc_session::config::{OutFileName, OutputType};
use rustc_span::{def_id::{DefId, LOCAL_CRATE}, symbol}; // DUMMY_SP, symbol::sym::test;
use rustc_smir::rustc_internal;
use stable_mir::{CrateItem,CrateDef,ItemKind,mir::{Body,LocalDecl,Terminator,TerminatorKind,Rvalue,visit::MirVisitor},ty::{Allocation,ForeignItemKind},mir::mono::{MonoItem,Instance,InstanceKind}}; // Symbol
use serde::{Serialize, Serializer};

// Structs for serializing extra details about mono items
// ======================================================

#[derive(Serialize, Clone)]
struct BodyDetails {
    pp: String,
}

fn get_body_details(body: &Body) -> BodyDetails {
  let mut v = Vec::new();
  let _ = body.dump(&mut v, "<omitted>");
  BodyDetails { pp: str::from_utf8(&v).unwrap().into() }
}

#[derive(Serialize, Clone)]
struct GenericData(Vec<(String,String)>); // Alternatively, GenericData<'a>(Vec<(&'a Generics,GenericPredicates<'a>)>);

fn generic_data(tcx: TyCtxt<'_>, id: DefId) -> GenericData {
     let mut v = Vec::new();
     let mut next_id = Some(id);
     while let Some(curr_id) = next_id {
        let params = tcx.generics_of(curr_id);
        let preds  = tcx.predicates_of(curr_id);
        if params.parent != preds.parent { panic!("Generics and predicates parent ids are distinct"); }
        v.push((format!("{:#?}", params), format!("{:#?}", preds)));
        next_id = params.parent;
     }
     v.reverse();
     return GenericData(v);
}

#[derive(Serialize, Clone)]
struct ItemDetails {
    // these fields only defined for fn items
    fn_instance_kind: Option<InstanceKind>,
    fn_item_kind: Option<ItemKind>,
    fn_body_details: Vec<BodyDetails>,
    // these fields defined for all items
    internal_kind: String,
    path: String,
    internal_ty: String,
    generic_data: GenericData,
}

// unwrap early binder in a default manner; panic on error
fn default_unwrap_early_binder<'tcx, T>(tcx: TyCtxt<'tcx>, id: DefId, v: EarlyBinder<'tcx, T>) -> T
  where T: TypeFoldable<TyCtxt<'tcx>>
{
  let v_copy = v.clone();
  match tcx.try_instantiate_and_normalize_erasing_regions(GenericArgs::identity_for_item(tcx, id), tcx.param_env(id), v) {
      Ok(res) => return res,
      Err(err) => { println!("{:?}", err); v_copy.skip_binder() }
  }
}

fn print_type<'tcx>(tcx: TyCtxt<'tcx>, id: DefId, ty: EarlyBinder<'tcx, Ty<'tcx>>) -> String {
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

fn get_item_details(tcx: TyCtxt<'_>, id: DefId, fn_inst: Option<Instance>) -> Option<ItemDetails> {
  if debug_enabled() {
    Some(ItemDetails {
      fn_instance_kind: fn_inst.map(|i| i.kind),
      fn_item_kind: fn_inst.map(|i| CrateItem::try_from(i).ok()).flatten().map(|i| i.kind()),
      fn_body_details: if let Some(fn_inst) = fn_inst { get_bodies(tcx, &fn_inst).iter().map(get_body_details).collect() } else { vec![] },
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

fn get_foreign_module_details() -> Vec<(String, Vec<ForeignModule>)> {
  let mut crates = vec![stable_mir::local_crate()];
  crates.append(&mut stable_mir::external_crates());
  crates.into_iter().map(|krate| {
      ( krate.name.clone(),
        krate.foreign_modules().into_iter().map(|mod_def| {
          let fmod = mod_def.module();
          ForeignModule { name: mod_def.name(), items: fmod.items().into_iter().map(|def| ForeignItem { name: def.name(), kind: def.kind() }).collect() }
        }).collect::<Vec<_>>()
      )
  }).collect()
}

// Miscellaneous helper functions
// ==============================

macro_rules! def_env_var {
    ($fn_name:ident, $var_name:ident) => {
        fn $fn_name() -> bool {
            use std::sync::OnceLock;
            static VAR: OnceLock<bool> = OnceLock::new();
            *VAR.get_or_init(|| {
                std::env::var(stringify!($var_name)).is_ok()
            })
        }
    };
}

def_env_var!(debug_enabled,         DEBUG);
def_env_var!(link_items_enabled,    LINK_ITEMS);
def_env_var!(link_instance_enabled, LINK_INST);

// Possible input: sym::test
pub fn has_attr(tcx: TyCtxt<'_>, item: &stable_mir::CrateItem, attr: symbol::Symbol) -> bool {
   tcx.has_attr(rustc_internal::internal(tcx,item), attr)
}

fn mono_item_name(tcx: TyCtxt<'_>, item: &MonoItem) -> String {
  if let MonoItem::GlobalAsm(data) = item {
    hash(data).to_string()
  } else {
    mono_item_name_int(tcx, &rustc_internal::internal(tcx, item))
  }
}

fn mono_item_name_int<'a>(tcx: TyCtxt<'a>, item: &rustc_middle::mir::mono::MonoItem<'a>) -> String {
  item.symbol_name(tcx).name.into()
}

fn get_promoted(tcx: TyCtxt<'_>, inst: &Instance) -> Vec<Body> {
  let id = rustc_internal::internal(tcx, inst.def.def_id());
  if inst.has_body() { tcx.promoted_mir(id).into_iter().map(rustc_internal::stable).collect() } else { vec![] }
}

fn get_bodies(tcx: TyCtxt<'_>, inst: &Instance) -> Vec<Body> {
  if let Some(body) = inst.body() {
    let mut bodies = get_promoted(tcx, inst);
    bodies.insert(0, body);
    bodies
  } else {
    vec![]
  }
}

fn fn_inst_for_ty(ty: stable_mir::ty::Ty, direct_call: bool) -> Option<Instance> {
  ty.kind().fn_def().map(|(fn_def, args)| {
    if direct_call {
      Instance::resolve(fn_def, args)
    } else {
      Instance::resolve_for_fn_ptr(fn_def, args)
    }.ok()
  }).flatten()
}

fn def_id_to_inst(tcx: TyCtxt<'_>, id: stable_mir::DefId) -> Instance {
  let internal_id = rustc_internal::internal(tcx,id);
  let internal_inst = rustc_middle::ty::Instance::mono(tcx, internal_id);
  rustc_internal::stable(internal_inst)
}

fn take_any<K: Clone + std::hash::Hash + std::cmp::Eq, V>(map: &mut HashMap<K,V>) -> Option<(K,V)> {
  let key = map.keys().next()?.clone();
  map.remove(&key).map(|val| (key,val))
}

fn hash<T: std::hash::Hash>(obj: T) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::hash::DefaultHasher::new();
    obj.hash(&mut hasher);
    hasher.finish()
}

// Structs for serializing critical details about mono items
// =========================================================

#[derive(Serialize, Clone)]
enum MonoItemKind {
    MonoItemFn {
      name: String,
      id: stable_mir::DefId,
      body: Vec<Body>,
    },
    MonoItemStatic {
      name: String,
      id: stable_mir::DefId,
      allocation: Option<Allocation>,
    },
    MonoItemGlobalAsm {
      asm: String,
    },
}
#[derive(Serialize, Clone)]
struct Item {
    #[serde(skip)]
    mono_item: MonoItem,
    symbol_name: String,
    mono_item_kind: MonoItemKind,
    details: Option<ItemDetails>,
}

fn mk_item(tcx: TyCtxt<'_>, item: MonoItem, sym_name: String) -> Item {
  match item {
    MonoItem::Fn(inst) => {
      let id = inst.def.def_id();
      let name = inst.name();
      let internal_id = rustc_internal::internal(tcx,id);
      Item {
        mono_item: item,
        symbol_name: sym_name,
        mono_item_kind: MonoItemKind::MonoItemFn {
          name: name.clone(),
          id: id,
          body: get_bodies(tcx, &inst),
        },
        details: get_item_details(tcx, internal_id, Some(inst))
      }
    },
    MonoItem::Static(static_def) => {
      let internal_id = rustc_internal::internal(tcx,static_def.def_id());
      let alloc = match static_def.eval_initializer() {
          Ok(alloc) => Some(alloc),
          err       => { println!("StaticDef({:#?}).eval_initializer() failed with: {:#?}", static_def, err); None }
      };
      Item {
        mono_item: item,
        symbol_name: sym_name,
        mono_item_kind: MonoItemKind::MonoItemStatic {
          name: static_def.name(),
          id: static_def.def_id(),
          allocation: alloc,
        },
        details: get_item_details(tcx, internal_id, None),
      }
    },
    MonoItem::GlobalAsm(ref asm) => {
      let asm = format!("{:#?}", asm);
      Item {
        mono_item: item,
        symbol_name: sym_name,
        mono_item_kind: MonoItemKind::MonoItemGlobalAsm { asm },
        details: None,
      }
    }
  }
}

// Link-time resolution logic
// ==========================

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
enum FnSymType {
    NoOpSym(String),
    IntrinsicSym(String),
    NormalSym(String),
}

type FnSymInfo<'tcx> = (stable_mir::ty::Ty, middle::ty::InstanceKind<'tcx>, FnSymType);

fn fn_inst_sym<'tcx>(tcx: TyCtxt<'tcx>, ty: Option<stable_mir::ty::Ty>, inst: Option<&Instance>) -> Option<FnSymInfo<'tcx>> {
  use FnSymType::*;
  inst.map(|inst| {
    let ty = if let Some(ty) = ty { ty } else { inst.ty() };
    let kind = ty.kind();
    if kind.fn_def().is_some() {
      let internal_inst = rustc_internal::internal(tcx, inst);
      let sym_type = if inst.is_empty_shim() {
         NoOpSym(String::from(""))
      } else if let Some(intrinsic_name) = inst.intrinsic_name() {
         IntrinsicSym(intrinsic_name)
      } else {
         NormalSym(inst.mangled_name())
      };
      Some((ty, internal_inst.def, sym_type))
    } else {
      None
    }
  }).flatten()
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LinkMapKey<'tcx>(stable_mir::ty::Ty, Option<middle::ty::InstanceKind<'tcx>>);

impl Serialize for LinkMapKey<'_> {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
      S: Serializer,
  {
    use serde::ser::SerializeTuple;
    if link_instance_enabled() {
      let mut tup = serializer.serialize_tuple(2)?;
      tup.serialize_element(&self.0)?;
      tup.serialize_element(&format!("{:?}", self.1).as_str())?;
      tup.end()
    } else {
      <stable_mir::ty::Ty as Serialize>::serialize(&self.0, serializer)
    }
  }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ItemSource(u8);
const ITEM: u8 = 1 << 0;
const TERM: u8 = 1 << 1;
const FPTR: u8 = 1 << 2;

impl Serialize for ItemSource {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
      S: Serializer,
  {
      use serde::ser::SerializeSeq;
      let mut seq = serializer.serialize_seq(None)?;
      if self.0 & ITEM != 0u8 { seq.serialize_element(&"Item")? };
      if self.0 & TERM != 0u8 { seq.serialize_element(&"Term")? };
      if self.0 & FPTR != 0u8 { seq.serialize_element(&"Fptr")? };
      seq.end()
  }
}

#[derive(Serialize)]
pub enum AllocInfo {
    Function(stable_mir::mir::mono::Instance),
    VTable(stable_mir::ty::Ty, Option<stable_mir::ty::Binder<stable_mir::ty::ExistentialTraitRef>>),
    Static(stable_mir::mir::mono::StaticDef),
    Memory(stable_mir::ty::TyKind, stable_mir::ty::Allocation),
}
type LinkMap<'tcx> = HashMap<LinkMapKey<'tcx>, (ItemSource, FnSymType)>;
type AllocMap = HashMap<stable_mir::mir::alloc::AllocId, AllocInfo>;
type TyMap = HashMap<u64, (stable_mir::ty::TyKind, Option<stable_mir::abi::LayoutShape>)>;

struct InternedValueCollector<'tcx, 'local> {
  tcx: TyCtxt<'tcx>,
  sym: String,
  locals: &'local [LocalDecl],
  link_map: &'local mut LinkMap<'tcx>,
  visited_allocs: &'local mut AllocMap,
  visited_tys: &'local mut TyMap,
}

type InternedValues<'tcx> = (LinkMap<'tcx>, AllocMap, TyMap);

fn update_link_map<'tcx>(link_map: &mut LinkMap<'tcx>, fn_sym: Option<FnSymInfo<'tcx>>, source: ItemSource) {
  if fn_sym.is_none() { return }
  let (ty, kind, name) = fn_sym.unwrap();
  let new_val = (source, name);
  let key = if link_instance_enabled() { LinkMapKey(ty, Some(kind)) } else { LinkMapKey(ty, None) };
  if let Some(curr_val) = link_map.get_mut(&key.clone()) {
    if curr_val.1 != new_val.1 {
      panic!("Added inconsistent entries into link map! {:?} -> {:?}, {:?}", (ty, ty.kind().fn_def(), &kind), curr_val.1, new_val.1);
    }
    curr_val.0.0 |= new_val.0.0;
    if debug_enabled() {
      println!("Regenerated link map entry: {:?}:{:?} -> {:?}", &key, key.0.kind().fn_def(), new_val);
    }
  } else {
    if debug_enabled() {
      println!("Generated link map entry from call: {:?}:{:?} -> {:?}", &key, key.0.kind().fn_def(), new_val);
    }
    link_map.insert(key.clone(), new_val);
  }
}

fn get_prov_type(maybe_kind: Option<stable_mir::ty::TyKind>) -> Option<stable_mir::ty::TyKind> {
  use stable_mir::ty::RigidTy;
  // check for pointers
  let kind = if let Some(kind) = maybe_kind { kind } else { return None };
  if let Some(ty) = kind.builtin_deref(true) {
    return ty.ty.kind().into();
  }
  match kind.rigid().expect("Non-rigid-ty allocation found!") {
    RigidTy::Array(ty, _) | RigidTy::Slice(ty) => ty.kind().into(),
    RigidTy::FnPtr(_) => None,
    _ => todo!(),
  }
}

fn collect_alloc(val_collector: &mut InternedValueCollector, kind: Option<stable_mir::ty::TyKind>, val: stable_mir::mir::alloc::AllocId) {
    use stable_mir::mir::alloc::GlobalAlloc;
    let entry = val_collector.visited_allocs.entry(val);
    if matches!(entry, std::collections::hash_map::Entry::Occupied(_)) { return; }
    let global_alloc = GlobalAlloc::from(val);
    match global_alloc {
        GlobalAlloc::Memory(ref alloc) => {
            let pointed_kind = get_prov_type(kind);
            if debug_enabled() { println!("DEBUG: called collect_alloc: {:?}:{:?}:{:?}", val, pointed_kind, global_alloc); }
            entry.or_insert(AllocInfo::Memory(pointed_kind.clone().unwrap(), alloc.clone()));
            alloc.provenance.ptrs.iter().for_each(|(_, prov)| {
                collect_alloc(val_collector, pointed_kind.clone(), prov.0);
            });
        }
        GlobalAlloc::Static(def) => {
            assert!(kind.clone().unwrap().builtin_deref(true).is_some(), "Allocated pointer is not a built-in pointer type: {:?}", kind);
            entry.or_insert(AllocInfo::Static(def));
        },
        GlobalAlloc::VTable(ty, traitref) => {
            assert!(kind.clone().unwrap().builtin_deref(true).is_some(), "Allocated pointer is not a built-in pointer type: {:?}", kind);
            entry.or_insert(AllocInfo::VTable(ty, traitref));
        },
        GlobalAlloc::Function(inst) => {
            assert!(kind.unwrap().is_fn_ptr());
            entry.or_insert(AllocInfo::Function(inst));
        },
    };
}

fn collect_vec_tys(collector: &mut InternedValueCollector, tys: Vec<stable_mir::ty::Ty>) {
    tys.into_iter().for_each(|ty| collect_ty(collector, ty));
}

fn collect_arg_tys(collector: &mut InternedValueCollector, args: &stable_mir::ty::GenericArgs) {
    use stable_mir::ty::{GenericArgKind::*, TyConst, TyConstKind::*};
    for arg in args.0.iter() {
        match arg {
            Type(ty) => collect_ty(collector, *ty),
            Const(ty_const) => match ty_const.kind() {
                Value(ty, _) | ZSTValue(ty) => collect_ty(collector, *ty),
                _ => {}
            },
            _ => {}
        }
     }
}

fn collect_ty(val_collector: &mut InternedValueCollector, val: stable_mir::ty::Ty) {
    use stable_mir::ty::{GenericArgKind::*, RigidTy::*, TyConst, TyConstKind::*, TyKind::RigidTy};
    if val_collector.visited_tys.insert(hash(val), (val.kind(), val.layout().map(|l| l.shape()).ok())).is_some() {
        match val.kind() {
            RigidTy(Array(ty, _) | Pat(ty, _) | Slice(ty) | RawPtr(ty, _) | Ref(_, ty, _)) => {
                collect_ty(val_collector, ty)
            }
            RigidTy(Tuple(tys)) => collect_vec_tys(val_collector, tys),
            RigidTy(Adt(def, ref args)) => {
                for variant in def.variants_iter() {
                    for field in variant.fields() {
                        collect_ty(val_collector, field.ty());
                    }
                }
                collect_arg_tys(val_collector, args);
            }
            // FIXME: Would be good to grab the coroutine signature
            RigidTy(Coroutine(_, ref args, _) | CoroutineWitness(_, ref args)) => collect_arg_tys(val_collector, args),
            ref kind @ RigidTy(FnDef(_, ref args) | Closure(_, ref args)) => {
                collect_vec_tys(val_collector, kind.fn_sig().unwrap().value.inputs_and_output);
                collect_arg_tys(val_collector, args);
            }
            RigidTy(Foreign(def)) => match def.kind() {
                ForeignItemKind::Fn(def) => collect_vec_tys(val_collector, def.fn_sig().value.inputs_and_output),
                ForeignItemKind::Type(ty) => collect_ty(val_collector, ty),
                ForeignItemKind::Static(def) => collect_ty(val_collector, def.ty()),
            }
            _ => {}
        }
    }
}

impl MirVisitor for InternedValueCollector<'_, '_> {
  fn visit_terminator(&mut self, term: &Terminator, loc: stable_mir::mir::visit::Location) {
    use TerminatorKind::*;
    use stable_mir::mir::{Operand::Constant, ConstOperand};
    let fn_sym = match &term.kind {
      Call { func: Constant(ConstOperand { const_: cnst, .. }), args: _, .. } => {
        if *cnst.kind() != stable_mir::ty::ConstantKind::ZeroSized { return }
        let inst = fn_inst_for_ty(cnst.ty(), true).expect("Direct calls to functions must resolve to an instance");
        fn_inst_sym(self.tcx, Some(cnst.ty()), Some(&inst))
      }
      Drop { place, .. } => {
        let drop_ty = place.ty(self.locals).unwrap();
        let inst = Instance::resolve_drop_in_place(drop_ty);
        fn_inst_sym(self.tcx, None, Some(&inst))
      }
      _ => None
    };
    update_link_map(self.link_map, fn_sym, ItemSource(TERM));
    self.super_terminator(term, loc);
  }

  fn visit_rvalue(&mut self, rval: &Rvalue, loc: stable_mir::mir::visit::Location) {
    use stable_mir::mir::{PointerCoercion, CastKind};
    match rval {
      Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer), ref op, _) => {
        let inst = fn_inst_for_ty(op.ty(self.locals).unwrap(), false).expect("ReifyFnPointer Cast operand type does not resolve to an instance");
        let fn_sym = fn_inst_sym(self.tcx, None, Some(&inst));
        update_link_map(self.link_map, fn_sym, ItemSource(FPTR));
      }
      _ => {}
    };
    self.super_rvalue(rval, loc);
  }

  fn visit_mir_const(&mut self, constant: &stable_mir::ty::MirConst, loc: stable_mir::mir::visit::Location) {
    use stable_mir::ty::{ConstantKind, TyConst, TyConstKind};
    match constant.kind() {
      ConstantKind::Allocated(alloc) => {
        if debug_enabled() { println!("visited_mir_const::Allocated({:?}) as {:?}", alloc, constant.ty().kind()); }
        alloc.provenance.ptrs.iter().for_each(|(_offset, prov)| collect_alloc(self, Some(constant.ty().kind()), prov.0));
      },
      ConstantKind::Ty(ty_const) => {
        if let TyConstKind::Value(..) = ty_const.kind() {
          panic!("TyConstKind::Value");
        }
      },
      ConstantKind::Unevaluated(_) | ConstantKind::Param(_) | ConstantKind::ZeroSized => {}
    }
    self.super_mir_const(constant, loc);
  }

  fn visit_ty(&mut self, ty: &stable_mir::ty::Ty, _location: stable_mir::mir::visit::Location) {
     collect_ty(self, *ty);
  }
}

fn collect_interned_values<'tcx,'local>(tcx: TyCtxt<'tcx>, items: Vec<&'local MonoItem>) -> InternedValues<'tcx> {
  let mut calls_map = HashMap::new();
  let mut visited_tys = HashMap::new();
  let mut visited_allocs = HashMap::new();
  if link_items_enabled() {
    for item in items.iter() {
      if let MonoItem::Fn ( inst ) = item {
         update_link_map(&mut calls_map, fn_inst_sym(tcx, None, Some(inst)), ItemSource(ITEM))
      }
    }
  }
  for item in items.iter() {
    match &item {
      MonoItem::Fn( inst ) => {
         for body in get_bodies(tcx, inst).into_iter() {
           InternedValueCollector {
             tcx,
             sym: inst.mangled_name(),
             locals: body.locals(),
             link_map: &mut calls_map,
             visited_tys: &mut visited_tys,
             visited_allocs: &mut visited_allocs,
           }.visit_body(&body)
         }
      }
      MonoItem::Static(def) => {
         let inst = def_id_to_inst(tcx, def.def_id());
         for body in get_bodies(tcx, &inst).into_iter() {
           InternedValueCollector {
             tcx,
             sym: inst.mangled_name(),
             locals: &[],
             link_map: &mut calls_map,
             visited_tys: &mut visited_tys,
             visited_allocs: &mut visited_allocs,
           }.visit_body(&body)
         }
      }
      MonoItem::GlobalAsm(_) => {}
    }
  }
  (calls_map, visited_allocs, visited_tys)
}


// Collection Transitive Closure
// =============================

struct UnevaluatedConstCollector<'tcx, 'local> {
  tcx: TyCtxt<'tcx>,
  unevaluated_consts: &'local mut HashMap<stable_mir::ty::ConstDef, String>,
  processed_items: &'local mut HashMap<String, Item>,
  pending_items: &'local mut HashMap<String, Item>,
  current_item: u64,
}

impl MirVisitor for UnevaluatedConstCollector<'_,'_> {
  fn visit_mir_const(&mut self, constant: &stable_mir::ty::MirConst, _location: stable_mir::mir::visit::Location) {
    if let stable_mir::ty::ConstantKind::Unevaluated(uconst) = constant.kind() {
      let internal_def = rustc_internal::internal(self.tcx, uconst.def.def_id());
      let internal_args = rustc_internal::internal(self.tcx, uconst.args.clone());
      let maybe_inst = rustc_middle::ty::Instance::try_resolve(self.tcx, ParamEnv::reveal_all(), internal_def, internal_args);
      let inst = maybe_inst.ok().flatten().expect(format!("Failed to resolve mono item for {:?}", uconst).as_str());
      let internal_mono_item = rustc_middle::mir::mono::MonoItem::Fn(inst);
      let item_name = mono_item_name_int(self.tcx, &internal_mono_item);
      if ! (    self.processed_items.contains_key(&item_name)
             || self.pending_items.contains_key(&item_name)
             || self.current_item == hash(&item_name)
           )
      {
          if debug_enabled() { println!("Adding unevaluated const body for: {}", item_name); }
          self.unevaluated_consts.insert(uconst.def, item_name.clone());
          self.pending_items.insert(item_name.clone(), mk_item(self.tcx, rustc_internal::stable(internal_mono_item), item_name));
      }
    }
  }
}

fn collect_unevaluated_constant_items(tcx: TyCtxt<'_>, items: HashMap<String,Item>) -> (HashMap<stable_mir::ty::ConstDef,String>, Vec<Item>) {
  // setup collector prerequisites
  let mut unevaluated_consts = HashMap::new();
  let mut processed_items = HashMap::new();
  let mut pending_items = items;
  loop {
    // get next pending item
    let (curr_name, value) = if let Some(v) = take_any(&mut pending_items) { v } else { break };

    // skip item if it isn't a function
    let bodies = match value.mono_item_kind {
      MonoItemKind::MonoItemFn { ref body, .. } => body,
      _ => continue
    };

    // create new collector for function body
    let mut collector = UnevaluatedConstCollector {
      tcx,
      unevaluated_consts: &mut unevaluated_consts,
      processed_items: &mut processed_items,
      pending_items: &mut pending_items,
      current_item: hash(&curr_name),
    };

    // add each fresh collected constant to pending new items
    bodies.iter().for_each(|body| collector.visit_body(body));

    // move processed item into seen items
    processed_items.insert(curr_name.to_string(), value);
  }

  (unevaluated_consts, processed_items.drain().map(|(_name,item)| item).collect())
}

// Core item collection logic
// ==========================

fn mono_collect(tcx: TyCtxt<'_>) -> Vec<MonoItem> {
  let units = tcx.collect_and_partition_mono_items(()).1;
  units.iter().flat_map(|unit| {
    unit.items_in_deterministic_order(tcx).iter().map(|(internal_item, _)| rustc_internal::stable(internal_item)).collect::<Vec<_>>()
  }).collect()
}

fn collect_items(tcx: TyCtxt<'_>) -> HashMap<String, Item> {
  // get initial set of mono_items
  let items = mono_collect(tcx);
  items.iter().map(|item| {
      let name = mono_item_name(tcx, item);
      ( name.clone(), mk_item(tcx, item.clone(), name) )
  }).collect::<HashMap<_,_>>()
}

// Serialization Entrypoint
// ========================

fn emit_smir_internal(tcx: TyCtxt<'_>, writer: &mut dyn io::Write) {
  let local_crate = stable_mir::local_crate();
  let items = collect_items(tcx);
  let items_clone = items.clone();
  let (unevaluated_consts, mut items) = collect_unevaluated_constant_items(tcx, items);
  let (calls_map, visited_allocs, visited_tys) = collect_interned_values(tcx, items.iter().map(|i| &i.mono_item).collect::<Vec<_>>());

  // FIXME: We dump extra static items here --- this should be handled better
  for (_, alloc) in visited_allocs.iter() {
      if let AllocInfo::Static(def) = alloc {
          let mono_item = stable_mir::mir::mono::MonoItem::Fn(stable_mir::mir::mono::Instance::from(*def));
          let item_name = &mono_item_name(tcx, &mono_item);
          if !items_clone.contains_key(item_name) {
            println!("Items missing static with id {:?} and name {:?}", def, item_name);
            items.push(mk_item(tcx, mono_item, item_name.clone()));
          }
      }
  }

  let called_functions = calls_map.iter().map(|(k,(_,name))| (k,name)).collect::<Vec<_>>();
  let crate_id = tcx.stable_crate_id(LOCAL_CRATE).as_u64();
  let json_items = serde_json::to_value(&items).expect("serde_json mono items to value failed");
  write!(writer, "{{\"name\": {}, \"crate_id\": {}, \"allocs\": {},  \"functions\": {},  \"uneval_consts\": {}, \"items\": {}",
    serde_json::to_string(&local_crate.name).expect("serde_json string to json failed"),
    serde_json::to_string(&crate_id).expect("serde_json number to json failed"),
    serde_json::to_string(&visited_allocs).expect("serde_json global allocs to json failed"),
    serde_json::to_string(&called_functions).expect("serde_json functions to json failed"),
    serde_json::to_string(&unevaluated_consts).expect("serde_json unevaluated consts to json failed"),
    serde_json::to_string(&json_items).expect("serde_json mono items to json failed"),
  ).expect("Failed to write JSON to file");
  if debug_enabled() {
    let fn_sources = calls_map.iter().map(|(k,(source,_))| (k,source)).collect::<Vec<_>>();
    write!(writer, ",\"fn_sources\": {}, \"types\": {}, \"foreign_modules\": {}}}",
      serde_json::to_string(&fn_sources).expect("serde_json functions failed"),
      serde_json::to_string(&visited_tys).expect("serde_json tys failed"),
      serde_json::to_string(&get_foreign_module_details()).expect("foreign_module serialization failed"),
    ).expect("Failed to write JSON to file");
  } else {
    write!(writer, "}}").expect("Failed to write JSON to file");
  }
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
