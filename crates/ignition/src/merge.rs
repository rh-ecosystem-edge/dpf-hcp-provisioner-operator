//! Port of `github.com/coreos/ignition/v2/config/merge.MergeStructTranscribe` (the merge
//! result only; the transcript is not needed by the operator).

use std::collections::{HashMap, HashSet};

use crate::gotypes::{GoVal, StructDef, Ty, call_key, is_zero};

struct StructInfo {
    ignore_dups: &'static [&'static str],
    merged_keys: &'static [(&'static str, &'static str)],
    /// (handle, key) -> (child list name, index in that list)
    keys: HashMap<(String, String), (&'static str, usize)>,
}

impl StructInfo {
    fn ignore_field(&self, name: &str) -> bool {
        self.ignore_dups.contains(&name)
    }

    fn handle(&self, name: &'static str) -> &'static str {
        self.merged_keys
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .unwrap_or(name)
    }
}

fn new_struct_info(def: &'static StructDef, child: &[GoVal]) -> StructInfo {
    let mut info = StructInfo {
        ignore_dups: def.ignore_dups,
        merged_keys: def.merged_keys,
        keys: HashMap::new(),
    };
    for (f, v) in def.fields.iter().zip(child) {
        let Ty::Slice(sd) = f.ty else { continue };
        if info.ignore_field(f.name) {
            continue;
        }
        let handle = info.handle(f.name);
        for (j, item) in v.as_slice().iter().enumerate() {
            info.keys
                .insert((handle.to_string(), call_key(sd.elem, item)), (f.name, j));
        }
    }
    info
}

/// `mergeStruct`: parent and child must be values of struct type `def`.
pub(crate) fn merge_struct(def: &'static StructDef, parent: &GoVal, child: &GoVal) -> GoVal {
    let pf = parent.fields();
    let cf = child.fields();
    let info = new_struct_info(def, cf);
    let mut result = Vec::with_capacity(def.fields.len());
    for (i, f) in def.fields.iter().enumerate() {
        let (p, c) = (&pf[i], &cf[i]);
        let r = match f.ty {
            Ty::Str(_) | Ty::Int => c.clone(),
            Ty::Ptr(_) => {
                if matches!(c, GoVal::Ptr(None)) {
                    p.clone()
                } else {
                    c.clone()
                }
            }
            Ty::Struct(sd) => {
                if is_zero(c) {
                    p.clone()
                } else if is_zero(p) {
                    c.clone()
                } else {
                    merge_struct(sd, p, c)
                }
            }
            Ty::Slice(sd) => {
                let (pl, cl) = (p.as_slice(), c.as_slice());
                if info.ignore_field(f.name) {
                    if pl.len() + cl.len() == 0 {
                        GoVal::Slice(None)
                    } else {
                        GoVal::Slice(Some(pl.iter().chain(cl).cloned().collect()))
                    }
                } else if pl.len() + cl.len() == 0 {
                    GoVal::Slice(None)
                } else {
                    let mut out = Vec::with_capacity(pl.len() + cl.len());
                    let parent_keys: HashSet<String> =
                        pl.iter().map(|x| call_key(sd.elem, x)).collect();
                    let handle = info.handle(f.name);
                    for item in pl {
                        let key = call_key(sd.elem, item);
                        match info.keys.get(&(handle.to_string(), key)) {
                            Some((list, idx)) => {
                                if *list == f.name {
                                    // case 1: in child config in same list
                                    let child_item = &cf[i].as_slice()[*idx];
                                    match sd.elem {
                                        Ty::Struct(ed) => {
                                            if f.name == "HTTPHeaders"
                                                && matches!(
                                                    crate::gotypes::SRef::new(ed, child_item)
                                                        .get("Value"),
                                                    GoVal::Ptr(None)
                                                )
                                            {
                                                continue;
                                            }
                                            out.push(merge_struct(ed, item, child_item));
                                        }
                                        _ => out.push(child_item.clone()),
                                    }
                                }
                                // case 2: in child config in a different list: dropped
                            }
                            // case 3: not in child config
                            None => out.push(item.clone()),
                        }
                    }
                    for item in cl {
                        if !parent_keys.contains(&call_key(sd.elem, item)) {
                            out.push(item.clone());
                        }
                    }
                    GoVal::Slice(Some(out))
                }
            }
        };
        result.push(r);
    }
    GoVal::Struct(result)
}
