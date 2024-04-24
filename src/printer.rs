use std::io;
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;
extern crate rustc_smir;
extern crate stable_mir;
use rustc_hir::definitions::{DefPath};
use rustc_middle::ty::TyCtxt;
use rustc_span::def_id::DefId;
use rustc_smir::rustc_internal;
use stable_mir::{CrateDef,Symbol};

pub fn print_item(tcx: TyCtxt<'_>, item: &stable_mir::CrateItem, out: &mut io::Stdout) {
  let _ = item.emit_mir(out);
  println!("{:?}", item.body());
  for (idx, promoted) in tcx.promoted_mir(rustc_internal::internal(tcx,item.def_id())).into_iter().enumerate() {
    let promoted_body = rustc_internal::stable(promoted);
    let _ = promoted_body.dump(out, format!("promoted[{}:{}]", item.name(), idx).as_str());
    println!("{:?}", promoted_body);
  }
}

pub fn print_item_details(tcx: TyCtxt<'_>, item: &stable_mir::CrateItem) {
  // Internal Details
  //
  // get DefId for internal API use
  let id: DefId = rustc_internal::internal(tcx,item);
  // get DefPath for item
  let path: DefPath = tcx.def_path(id);
  // get string version of DefPath
  let path_str: String = tcx.def_path_str(id); 
  // get type, generic parameters, required predicates, layout
  let ty = tcx.type_of(id);
  let params = tcx.generics_of(id);
  let predicates = tcx.predicates_of(id);
  // let layout = tcx.layout_of(id);
  println!("===Internal===\nDefId: {:#?}\nDefPath: {:#?}\nDefPathStr: {}\nTy: {:#?}\nParams: {:#?}\nPreds: {:#?}",
           id, path, path_str, ty, params, predicates);

  // Stable Details
  //
  // get stable MIR kind
  let kind = item.kind();
  // get MIR Symbol
  let name: Symbol = item.name();
  println!("===SMIR===\nkind: {:#?}\nname: {:#?}",
           kind, name);
}

pub fn print_all_items(tcx: TyCtxt<'_>) {
  let mut out = io::stdout();
  for item in stable_mir::all_local_items().iter() {
      print_item(tcx, item, &mut out);
  }
}

pub fn print_all_items_verbose(tcx: TyCtxt<'_>) {
  let mut out = io::stdout();
  for item in stable_mir::all_local_items().iter() {
      print_item_details(tcx, item);
      print_item(tcx, item, &mut out);
  }
}
