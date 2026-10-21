// VEX-style language for the Wrangle node, modelled on Houdini's VEX: a lexer,
// a parser that resolves variables to frame slots, and a tree-walking
// interpreter that runs the snippet once per element (detail, primitive, point
// or vertex). Supports typed locals, arrays, strings, control flow, user
// functions and geometry functions; writes made through set*attrib, add* and
// remove* are deferred until every element has run, as in Houdini.

use super::{hash01, Mesh, Vec3};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

// Loop iterations plus function calls allowed per cook, so a runaway loop cannot freeze the app.
#[cfg(not(test))]
const STEP_LIMIT: u64 = 50_000_000;
#[cfg(test)]
const STEP_LIMIT: u64 = 200_000;
const MAX_CALL_DEPTH: usize = 64;
const MAX_LOG_LINES: usize = 500;

type R<T> = Result<T, String>;

// ============================================================================
// Values and types
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    Int,
    Float,
    Vector,
    Str,
    IntArr,
    FloatArr,
    VecArr,
    StrArr,
    // vector4 (also quaternions), matrix3 and matrix (4x4); no arrays of these.
    Vec4,
    Mat3,
    Mat4,
}

impl Ty {
    fn array(self) -> Ty {
        match self {
            Ty::Int => Ty::IntArr,
            Ty::Float => Ty::FloatArr,
            Ty::Vector => Ty::VecArr,
            Ty::Str => Ty::StrArr,
            other => other,
        }
    }

    fn element(self) -> Option<Ty> {
        match self {
            Ty::IntArr => Some(Ty::Int),
            Ty::FloatArr => Some(Ty::Float),
            Ty::VecArr => Some(Ty::Vector),
            Ty::StrArr => Some(Ty::Str),
            _ => None,
        }
    }

    fn default_value(self) -> Value {
        match self {
            Ty::Int => Value::Int(0),
            Ty::Float => Value::Float(0.0),
            Ty::Vector => Value::Vec([0.0; 3]),
            Ty::Str => Value::Str("".into()),
            Ty::Vec4 => Value::Vec4([0.0; 4]),
            // Matrices start as the identity, as in VEX.
            Ty::Mat3 => Value::Mat3(crate::vexmath::m3_ident()),
            Ty::Mat4 => Value::Mat4(crate::vexmath::m4_ident()),
            _ => Value::Arr(Rc::new(Vec::new())),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Vector => "vector",
            Ty::Str => "string",
            Ty::IntArr => "int[]",
            Ty::FloatArr => "float[]",
            Ty::VecArr => "vector[]",
            Ty::StrArr => "string[]",
            Ty::Vec4 => "vector4",
            Ty::Mat3 => "matrix3",
            Ty::Mat4 => "matrix",
        }
    }
}

#[derive(Clone, Debug)]
enum Value {
    Int(i32),
    Float(f32),
    Vec([f32; 3]),
    Str(Rc<str>),
    Arr(Rc<Vec<Value>>),
    Vec4([f32; 4]),
    Mat3([f32; 9]),
    Mat4([f32; 16]),
}

impl Default for Value {
    fn default() -> Self {
        Value::Int(0)
    }
}

impl Value {
    fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Vec(_) => "vector",
            Value::Str(_) => "string",
            Value::Arr(_) => "array",
            Value::Vec4(_) => "vector4",
            Value::Mat3(_) => "matrix3",
            Value::Mat4(_) => "matrix",
        }
    }

    // The flat components of a vector4 or matrix.
    fn floats(&self) -> Option<&[f32]> {
        match self {
            Value::Vec4(v) => Some(v),
            Value::Mat3(m) => Some(m),
            Value::Mat4(m) => Some(m),
            _ => None,
        }
    }

    fn vec4(&self) -> R<[f32; 4]> {
        match self {
            Value::Vec4(v) => Ok(*v),
            Value::Int(_) | Value::Float(_) => Ok([self.num()?; 4]),
            Value::Vec(v) => Ok([v[0], v[1], v[2], 0.0]),
            Value::Arr(items) if items.len() == 4 => Ok([items[0].num()?, items[1].num()?, items[2].num()?, items[3].num()?]),
            other => Err(format!("expected a vector4, got {}", other.type_name())),
        }
    }

    fn mat3(&self) -> R<[f32; 9]> {
        match self {
            Value::Mat3(m) => Ok(*m),
            Value::Mat4(m) => Ok(crate::vexmath::m4_to_m3(m)),
            Value::Int(_) | Value::Float(_) => Ok(crate::vexmath::m3_ident().map(|c| c * self.num().unwrap_or(0.0))),
            Value::Arr(items) if items.len() == 9 => {
                let v = items.iter().map(Value::num).collect::<R<Vec<f32>>>()?;
                Ok(v.try_into().unwrap())
            }
            other => Err(format!("expected a matrix3, got {}", other.type_name())),
        }
    }

    fn mat4(&self) -> R<[f32; 16]> {
        match self {
            Value::Mat4(m) => Ok(*m),
            Value::Mat3(m) => Ok(crate::vexmath::m3_to_m4(m)),
            Value::Int(_) | Value::Float(_) => Ok(crate::vexmath::m4_ident().map(|c| c * self.num().unwrap_or(0.0))),
            Value::Arr(items) if items.len() == 16 => {
                let v = items.iter().map(Value::num).collect::<R<Vec<f32>>>()?;
                Ok(v.try_into().unwrap())
            }
            other => Err(format!("expected a matrix, got {}", other.type_name())),
        }
    }

    fn num(&self) -> R<f32> {
        match self {
            Value::Int(i) => Ok(*i as f32),
            Value::Float(f) => Ok(*f),
            Value::Vec(v) => Ok(v[0]),
            Value::Str(s) => Err(format!("expected a number, got string \"{s}\"")),
            Value::Arr(_) => Err("expected a number, got an array".into()),
            other => Err(format!("expected a number, got a {}", other.type_name())),
        }
    }

    fn int(&self) -> R<i32> {
        match self {
            Value::Int(i) => Ok(*i),
            other => other.num().map(|f| f as i32),
        }
    }

    fn vec(&self) -> R<[f32; 3]> {
        match self {
            Value::Int(i) => Ok([*i as f32; 3]),
            Value::Float(f) => Ok([*f; 3]),
            Value::Vec(v) => Ok(*v),
            Value::Arr(items) if items.len() == 3 => Ok([items[0].num()?, items[1].num()?, items[2].num()?]),
            Value::Arr(items) => Err(format!("expected a vector, got an array of {} items", items.len())),
            Value::Str(s) => Err(format!("expected a vector, got string \"{s}\"")),
            // A vector4 shortens to its first three components, as in VEX.
            Value::Vec4(v) => Ok([v[0], v[1], v[2]]),
            other => Err(format!("expected a vector, got a {}", other.type_name())),
        }
    }

    fn string(&self) -> R<Rc<str>> {
        match self {
            Value::Str(s) => Ok(s.clone()),
            other => Err(format!("expected a string, got {}", other.type_name())),
        }
    }

    fn items(&self) -> R<Rc<Vec<Value>>> {
        match self {
            Value::Arr(items) => Ok(items.clone()),
            Value::Vec(v) => Ok(Rc::new(v.iter().map(|c| Value::Float(*c)).collect())),
            other => Err(format!("expected an array, got {}", other.type_name())),
        }
    }

    fn is_vector_like(&self) -> bool {
        match self {
            Value::Vec(_) => true,
            Value::Arr(items) => items.len() == 3,
            _ => false,
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Vec(v) => v.iter().any(|c| *c != 0.0),
            Value::Str(s) => !s.is_empty(),
            Value::Arr(items) => !items.is_empty(),
            other => other.floats().is_some_and(|f| f.iter().any(|c| *c != 0.0)),
        }
    }

    fn display(&self) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Vec(v) => format!("{{{}, {}, {}}}", v[0], v[1], v[2]),
            Value::Str(s) => s.to_string(),
            Value::Arr(items) => format!("{{{}}}", items.iter().map(Value::display).collect::<Vec<_>>().join(", ")),
            other => format!("{{{}}}", other.floats().unwrap_or(&[]).iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")),
        }
    }

    // A zero of the same kind, used to pad arrays.
    fn zero_like(&self) -> Value {
        match self {
            Value::Int(_) => Value::Int(0),
            Value::Float(_) => Value::Float(0.0),
            Value::Vec(_) => Value::Vec([0.0; 3]),
            Value::Str(_) => Value::Str("".into()),
            Value::Arr(_) => Value::Arr(Rc::new(Vec::new())),
            Value::Vec4(_) => Value::Vec4([0.0; 4]),
            Value::Mat3(_) => Value::Mat3([0.0; 9]),
            Value::Mat4(_) => Value::Mat4([0.0; 16]),
        }
    }

    fn convert(self, ty: Ty) -> R<Value> {
        let fail = |v: &Value| Err(format!("cannot convert {} to {}", v.type_name(), ty.name()));
        match ty {
            Ty::Int => match self {
                Value::Int(_) => Ok(self),
                Value::Float(_) | Value::Vec(_) => Ok(Value::Int(self.num()? as i32)),
                _ => fail(&self),
            },
            Ty::Float => match self {
                Value::Float(_) => Ok(self),
                Value::Int(_) | Value::Vec(_) => Ok(Value::Float(self.num()?)),
                _ => fail(&self),
            },
            Ty::Vector => match self {
                Value::Vec(_) => Ok(self),
                Value::Int(_) | Value::Float(_) | Value::Arr(_) => Ok(Value::Vec(self.vec()?)),
                _ => fail(&self),
            },
            Ty::Str => match self {
                Value::Str(_) => Ok(self),
                _ => fail(&self),
            },
            Ty::Vec4 => self.vec4().map(Value::Vec4),
            Ty::Mat3 => self.mat3().map(Value::Mat3),
            Ty::Mat4 => self.mat4().map(Value::Mat4),
            _ => {
                let element = ty.element().expect("array type");
                let items = match self {
                    Value::Arr(items) => items,
                    Value::Vec(v) if element != Ty::Str => Rc::new(v.iter().map(|c| Value::Float(*c)).collect()),
                    _ => return fail(&self),
                };
                let matches = |v: &Value| {
                    matches!(
                        (element, v),
                        (Ty::Int, Value::Int(_)) | (Ty::Float, Value::Float(_)) | (Ty::Vector, Value::Vec(_)) | (Ty::Str, Value::Str(_))
                    )
                };
                if items.iter().all(matches) {
                    return Ok(Value::Arr(items));
                }
                let converted = items.iter().cloned().map(|v| v.convert(element)).collect::<R<Vec<_>>>()?;
                Ok(Value::Arr(Rc::new(converted)))
            }
        }
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (a.floats(), b.floats()) {
        return x == y;
    }
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Arr(x), Value::Arr(y)) if !(a.is_vector_like() && b.is_vector_like()) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| values_equal(p, q))
        }
        (Value::Str(_), _) | (_, Value::Str(_)) => false,
        _ if a.is_vector_like() || b.is_vector_like() => matches!((a.vec(), b.vec()), (Ok(x), Ok(y)) if x == y),
        _ => matches!((a.num(), b.num()), (Ok(x), Ok(y)) if x == y),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

fn binop(op: BinOp, a: Value, b: Value) -> R<Value> {
    use BinOp::*;
    match op {
        Eq => return Ok(Value::Int(values_equal(&a, &b) as i32)),
        Ne => return Ok(Value::Int(!values_equal(&a, &b) as i32)),
        Lt | Le | Gt | Ge => {
            let ordering = match (&a, &b) {
                (Value::Str(x), Value::Str(y)) => x.cmp(y),
                _ if a.is_vector_like() || b.is_vector_like() || matches!(a, Value::Str(_)) || matches!(b, Value::Str(_)) => {
                    return Err(format!("cannot compare {} with {}", a.type_name(), b.type_name()));
                }
                _ => a.num()?.partial_cmp(&b.num()?).unwrap_or(std::cmp::Ordering::Equal),
            };
            let result = match op {
                Lt => ordering.is_lt(),
                Le => ordering.is_le(),
                Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            };
            return Ok(Value::Int(result as i32));
        }
        _ => {}
    }
    if a.floats().is_some() || b.floats().is_some() {
        return matrix_binop(op, &a, &b);
    }
    if op == Add && (matches!(a, Value::Str(_)) || matches!(b, Value::Str(_))) {
        return Ok(Value::Str(format!("{}{}", a.display(), b.display()).into()));
    }
    if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
        let (x, y) = (*x, *y);
        return Ok(Value::Int(match op {
            Add => x.wrapping_add(y),
            Sub => x.wrapping_sub(y),
            Mul => x.wrapping_mul(y),
            Div => if y == 0 { 0 } else { x.wrapping_div(y) },
            _ => if y == 0 { 0 } else { x.wrapping_rem(y) },
        }));
    }
    let apply = |x: f32, y: f32| match op {
        Add => x + y,
        Sub => x - y,
        Mul => x * y,
        Div => if y == 0.0 { 0.0 } else { x / y },
        _ => if y == 0.0 { 0.0 } else { x % y },
    };
    if a.is_vector_like() || b.is_vector_like() {
        let (x, y) = (a.vec()?, b.vec()?);
        return Ok(Value::Vec([apply(x[0], y[0]), apply(x[1], y[1]), apply(x[2], y[2])]));
    }
    if matches!(a, Value::Arr(_)) || matches!(b, Value::Arr(_)) {
        return Err(format!("cannot do arithmetic on {} and {}", a.type_name(), b.type_name()));
    }
    Ok(Value::Float(apply(a.num()?, b.num()?)))
}

// Arithmetic with vector4s and matrices: v * M (row vector), M * v (column),
// M * M, componentwise + and -, and scaling by numbers.
fn matrix_binop(op: BinOp, a: &Value, b: &Value) -> R<Value> {
    use crate::vexmath as vm;
    use BinOp::*;
    let scalar = |v: &Value| matches!(v, Value::Int(_) | Value::Float(_));
    let lanes = |x: &[f32], y: &[f32]| -> Vec<f32> {
        x.iter()
            .zip(y)
            .map(|(p, q)| match op {
                Add => p + q,
                Sub => p - q,
                Mul => p * q,
                Div => if *q == 0.0 { 0.0 } else { p / q },
                _ => if *q == 0.0 { 0.0 } else { p % q },
            })
            .collect()
    };
    let same_shape = |x: &[f32]| -> R<Value> {
        Ok(match x.len() {
            4 => Value::Vec4(x.try_into().unwrap()),
            9 => Value::Mat3(x.try_into().unwrap()),
            _ => Value::Mat4(x.try_into().unwrap()),
        })
    };
    match (a, b) {
        // Scaling by a number.
        (m, s) if scalar(s) && m.floats().is_some() => {
            let k = s.num()?;
            same_shape(&lanes(m.floats().unwrap(), &vec![k; m.floats().unwrap().len()]))
        }
        (s, m) if scalar(s) && m.floats().is_some() && matches!(op, Add | Mul) => {
            let k = s.num()?;
            same_shape(&lanes(&vec![k; m.floats().unwrap().len()], m.floats().unwrap()))
        }
        (Value::Vec4(x), Value::Vec4(y)) => same_shape(&lanes(x, y)),
        // Transforming vectors.
        (v, Value::Mat3(m)) if op == Mul && v.is_vector_like() => Ok(Value::Vec(vm::v_m3(v.vec()?, m))),
        (v, Value::Mat4(m)) if op == Mul && v.is_vector_like() => Ok(Value::Vec(vm::v_m4(v.vec()?, m))),
        (Value::Mat3(m), v) if op == Mul && v.is_vector_like() => {
            Ok(Value::Vec(vm::v_m3(v.vec()?, &vm::transpose::<3>(m).try_into().unwrap())))
        }
        (Value::Mat4(m), v) if op == Mul && v.is_vector_like() => {
            Ok(Value::Vec(vm::v_m4(v.vec()?, &vm::transpose::<4>(m).try_into().unwrap())))
        }
        // Matrix products; a matrix3 with a matrix becomes a matrix.
        (Value::Mat3(x), Value::Mat3(y)) if op == Mul => Ok(Value::Mat3(vm::m3_mul(x, y))),
        (x @ (Value::Mat3(_) | Value::Mat4(_)), y @ (Value::Mat3(_) | Value::Mat4(_))) if op == Mul => Ok(Value::Mat4(vm::m4_mul(&x.mat4()?, &y.mat4()?))),
        (x, y) if matches!(op, Add | Sub) && x.floats().map(<[f32]>::len) == y.floats().map(<[f32]>::len) => {
            same_shape(&lanes(x.floats().unwrap(), y.floats().unwrap()))
        }
        _ => Err(format!("cannot combine {} and {} with that operator", a.type_name(), b.type_name())),
    }
}

fn normalize_index(i: i32, len: usize) -> Option<usize> {
    let index = if i < 0 { len as i64 + i as i64 } else { i as i64 };
    (index >= 0).then_some(index as usize)
}

fn component(v: &Value, c: usize) -> R<Value> {
    match v {
        Value::Vec(x) => Ok(Value::Float(x[c])),
        Value::Int(_) | Value::Float(_) => Ok(v.clone()),
        Value::Arr(_) => index_read(v, c as i32),
        Value::Str(_) => Err("strings have no .x/.y/.z components".into()),
        Value::Vec4(x) => Ok(Value::Float(x[c.min(3)])),
        _ => Err(format!("a {} has no .x/.y/.z components (use getcomp)", v.type_name())),
    }
}

fn set_component(base: Value, c: usize, f: f32) -> R<Value> {
    if let Value::Vec4(mut v) = base {
        v[c.min(3)] = f;
        return Ok(Value::Vec4(v));
    }
    if matches!(base, Value::Mat3(_) | Value::Mat4(_)) {
        return Err("matrices have no .x/.y/.z components (use setcomp)".into());
    }
    if matches!(base, Value::Str(_)) {
        return Err("strings have no .x/.y/.z components".into());
    }
    if matches!(&base, Value::Arr(items) if items.len() != 3) {
        return index_write(base, c as i32, Value::Float(f)).map(|(v, _)| v);
    }
    let mut v = base.vec()?;
    v[c] = f;
    Ok(Value::Vec(v))
}

fn index_read(base: &Value, i: i32) -> R<Value> {
    match base {
        Value::Arr(items) => Ok(normalize_index(i, items.len())
            .and_then(|i| items.get(i).cloned())
            .unwrap_or_else(|| items.first().map_or(Value::Int(0), Value::zero_like))),
        Value::Vec(v) => Ok(Value::Float(normalize_index(i, 3).and_then(|i| v.get(i).copied()).unwrap_or(0.0))),
        Value::Vec4(v) => Ok(Value::Float(normalize_index(i, 4).and_then(|i| v.get(i).copied()).unwrap_or(0.0))),
        Value::Str(s) => Ok(Value::Str(
            normalize_index(i, s.chars().count())
                .and_then(|i| s.chars().nth(i))
                .map(|c| c.to_string())
                .unwrap_or_default()
                .into(),
        )),
        other => Err(format!("cannot index a {} with [ ]", other.type_name())),
    }
}

fn index_write(base: Value, i: i32, v: Value) -> R<(Value, Value)> {
    match base {
        Value::Arr(items) => {
            let mut items = Rc::try_unwrap(items).unwrap_or_else(|shared| (*shared).clone());
            let index = normalize_index(i, items.len()).ok_or_else(|| format!("array index {i} is out of range"))?;
            if index >= items.len() {
                // Writing past the end grows the array, as in VEX.
                items.resize(index + 1, v.zero_like());
            }
            items[index] = v.clone();
            Ok((Value::Arr(Rc::new(items)), v))
        }
        Value::Vec(mut x) => {
            let index = normalize_index(i, 3).filter(|i| *i < 3).ok_or_else(|| format!("vector index {i} is out of range (0-2)"))?;
            x[index] = v.num()?;
            Ok((Value::Vec(x), v))
        }
        Value::Vec4(mut x) => {
            let index = normalize_index(i, 4).filter(|i| *i < 4).ok_or_else(|| format!("vector4 index {i} is out of range (0-3)"))?;
            x[index] = v.num()?;
            Ok((Value::Vec4(x), v))
        }
        other => Err(format!("cannot index a {} with [ ]", other.type_name())),
    }
}

// ============================================================================
// Lexer
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Int(i32),
    Float(f32),
    Str(String),
    Ident(String),
    Attr(Option<char>, String),
    P(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    line: usize,
    col: usize,
    end_col: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VexError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl fmt::Display for VexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, col {}: {}", self.line, self.col, self.message)
    }
}

const PUNCTS: [&str; 33] = [
    "++", "--", "+=", "-=", "*=", "/=", "%=", "==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "=", "<", ">",
    "!", "?", ":", ";", ",", ".", "(", ")", "{", "}", "[", "]",
];

fn lex(src: &str) -> Result<Vec<Token>, VexError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let (mut i, mut line, mut line_start) = (0usize, 1usize, 0usize);
    let error = |line: usize, col: usize, message: String| Err(VexError { line, col, message });
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied().unwrap_or('\0');
        let col = i - line_start + 1;
        if c == '\n' {
            i += 1;
            line += 1;
            line_start = i;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && next == '/' || c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == '*' {
            let (start_line, start_col) = (line, col);
            i += 2;
            loop {
                if i >= chars.len() {
                    return error(start_line, start_col, "unterminated /* comment".into());
                }
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    i += 2;
                    break;
                }
                if chars[i] == '\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
            continue;
        }
        let start = i;
        let after_value = matches!(
            out.last().map(|t| &t.tok),
            Some(Tok::Ident(_) | Tok::Attr(..) | Tok::Int(_) | Tok::Float(_) | Tok::P(")") | Tok::P("]"))
        );
        let tok = if (c == '3' || c == '4') && next == '@' && !after_value {
            // 3@xform (matrix3) and 4@xform (matrix) attribute bindings.
            i += 2;
            let name_start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if i == name_start {
                return error(line, col, "expected an attribute name after '@'".into());
            }
            Tok::Attr(Some(c), chars[name_start..i].iter().collect())
        } else if c.is_ascii_digit() || (c == '.' && next.is_ascii_digit() && !after_value) {
            let mut is_float = false;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                is_float |= chars[i] == '.';
                i += 1;
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    is_float = true;
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            if i < chars.len() && (chars[i] == 'f' || chars[i] == 'F') {
                is_float = true;
                i += 1;
            }
            match (is_float, text.parse::<i32>()) {
                (false, Ok(n)) => Tok::Int(n),
                _ => match text.parse::<f32>() {
                    Ok(f) => Tok::Float(f),
                    Err(_) => return error(line, col, format!("invalid number '{text}'")),
                },
            }
        } else if c == '"' || c == '\'' {
            let quote = c;
            let mut text = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None | Some('\n') => return error(line, col, "unterminated string".into()),
                    Some(&ch) if ch == quote => {
                        i += 1;
                        break;
                    }
                    Some('\\') => {
                        let escaped = chars.get(i + 1).copied().unwrap_or('\\');
                        text.push(match escaped {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                        i += 2;
                    }
                    Some(&ch) => {
                        text.push(ch);
                        i += 1;
                    }
                }
            }
            Tok::Str(text)
        } else if c == '@' {
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if i == start + 1 {
                return error(line, col, "expected an attribute name after '@'".into());
            }
            Tok::Attr(None, chars[start + 1..i].iter().collect())
        } else if c.is_ascii_alphabetic() || c == '_' {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            // Typed attribute bindings: f@mass, i@id, v@vel, s@name ...
            if name.len() == 1 && chars.get(i) == Some(&'@') {
                let name_start = i + 1;
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                if i == name_start {
                    return error(line, col, "expected an attribute name after '@'".into());
                }
                Tok::Attr(name.chars().next(), chars[name_start..i].iter().collect())
            } else {
                Tok::Ident(name)
            }
        } else {
            let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
            let found = PUNCTS
                .iter()
                .find(|p| p.len() == 2 && **p == two)
                .or_else(|| PUNCTS.iter().find(|p| p.len() == 1 && p.starts_with(c)));
            match found {
                Some(p) => {
                    i += p.len();
                    Tok::P(p)
                }
                None => return error(line, col, format!("unexpected character '{c}'")),
            }
        };
        out.push(Token { tok, line, col, end_col: i - line_start + 1 });
    }
    let col = chars.len() - line_start + 1;
    out.push(Token { tok: Tok::Eof, line, col, end_col: col });
    Ok(out)
}

fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Int(i) => format!("number {i}"),
        Tok::Float(f) => format!("number {f}"),
        Tok::Str(_) => "a string".into(),
        Tok::Ident(name) => format!("'{name}'"),
        Tok::Attr(_, name) => format!("@{name}"),
        Tok::P(p) => format!("'{p}'"),
        Tok::Eof => "the end of the code".into(),
    }
}

// ============================================================================
// Syntax tree
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
enum AttrKind {
    P,
    N,
    Ptnum,
    Primnum,
    Vtxnum,
    Numpt,
    Numprim,
    Numvtx,
    Elemnum,
    Numelem,
    Time,
    TimeInc,
    Frame,
    Other,
}

impl AttrKind {
    fn of(name: &str) -> AttrKind {
        match name {
            "P" => AttrKind::P,
            "N" => AttrKind::N,
            "ptnum" => AttrKind::Ptnum,
            "primnum" => AttrKind::Primnum,
            "vtxnum" => AttrKind::Vtxnum,
            "numpt" => AttrKind::Numpt,
            "numprim" => AttrKind::Numprim,
            "numvtx" => AttrKind::Numvtx,
            "elemnum" => AttrKind::Elemnum,
            "numelem" => AttrKind::Numelem,
            "Time" | "SimTime" => AttrKind::Time,
            "TimeInc" => AttrKind::TimeInc,
            "Frame" | "SimFrame" => AttrKind::Frame,
            _ => AttrKind::Other,
        }
    }

    fn read_only(self) -> bool {
        !matches!(self, AttrKind::P | AttrKind::N | AttrKind::Other)
    }
}

// Type of well-known attributes when no f@/i@/v@ prefix is given.
fn known_hint(name: &str) -> Option<Ty> {
    match name {
        "P" | "N" | "Cd" | "v" | "up" | "rest" | "scale" | "force" | "accel" | "uv" | "w" => Some(Ty::Vector),
        "ptnum" | "primnum" | "vtxnum" | "numpt" | "numprim" | "numvtx" | "elemnum" | "numelem" | "id" => Some(Ty::Int),
        "Time" | "SimTime" | "TimeInc" | "Frame" | "SimFrame" | "pscale" => Some(Ty::Float),
        "orient" | "rot" => Some(Ty::Vec4),
        _ => None,
    }
}

// Attribute names usable without '@' (the original wrangle syntax).
const LEGACY_ATTRS: [&str; 11] = ["P", "N", "Cd", "Time", "ptnum", "primnum", "vtxnum", "numpt", "numprim", "numvtx", "pscale"];

#[derive(Clone, Debug)]
struct AttrRef {
    name: String,
    kind: AttrKind,
    hint: Option<Ty>,
    // Assigned somewhere in the code (used to warn about reads of missing attributes).
    written: bool,
}

#[derive(Clone, Debug)]
enum Expr {
    Const(Value),
    Local(usize),
    Attr(usize),
    List(Vec<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Assign(Box<Place>, Option<BinOp>, Box<Expr>),
    // ++/--: delta, and whether the new (prefix) or old (postfix) value is the result.
    Step(Box<Place>, i32, bool),
    Member(Box<Expr>, usize),
    Index(Box<Expr>, Box<Expr>),
    // a[start:end:step], each part optional.
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>, Option<Box<Expr>>),
    // Name, arguments, assignable argument places (array-mutating builtins), vector return context.
    Builtin(&'static str, Vec<Expr>, Vec<Option<Place>>, bool),
    // User function index, arguments, argument places written back (VEX passes by reference).
    Call(usize, Vec<Expr>, Vec<Option<Place>>),
    // A call to an overloaded function: the candidates, picked by argument types at run time.
    CallOverload(Vec<usize>, Vec<Expr>, Vec<Option<Place>>),
}

#[derive(Clone, Debug)]
enum Place {
    Local(usize),
    Attr(usize),
    Member(Box<Place>, usize),
    Index(Box<Place>, Box<Expr>),
}

fn to_place(e: &Expr) -> Option<Place> {
    match e {
        Expr::Local(slot) => Some(Place::Local(*slot)),
        Expr::Attr(id) => Some(Place::Attr(*id)),
        Expr::Member(base, c) => Some(Place::Member(Box::new(to_place(base)?), *c)),
        Expr::Index(base, index) => Some(Place::Index(Box::new(to_place(base)?), index.clone())),
        _ => None,
    }
}

fn root_attr(place: &Place) -> Option<usize> {
    match place {
        Place::Attr(id) => Some(*id),
        Place::Member(base, _) | Place::Index(base, _) => root_attr(base),
        Place::Local(_) => None,
    }
}

#[derive(Clone, Debug)]
struct Stmt {
    line: usize,
    kind: StmtKind,
}

#[derive(Clone, Debug)]
enum StmtKind {
    Expr(Expr),
    Decl(usize, Option<Expr>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    For(Vec<Stmt>, Option<Expr>, Option<Expr>, Box<Stmt>),
    Foreach(Option<usize>, usize, Expr, Box<Stmt>),
    Block(Vec<Stmt>),
    Break,
    Continue,
    Return(Option<Expr>),
}

#[derive(Clone, Debug)]
struct Function {
    name: String,
    ret: Option<Ty>,
    param_types: Vec<Ty>,
    // Declared type of every local slot; None for implicitly declared (dynamic) locals.
    slot_types: Vec<Option<Ty>>,
    body: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub struct Program {
    main: Function,
    functions: Vec<Function>,
    attrs: Vec<AttrRef>,
}

// ============================================================================
// Built-in function table
// ============================================================================

// (name, min args, max args, group, signature)
const VARIADIC: usize = usize::MAX;
pub const BUILTINS: &[(&str, usize, usize, &str, &str)] = &[
    ("abs", 1, 1, "Math", "abs(x)"),
    ("sign", 1, 1, "Math", "sign(x)"),
    ("floor", 1, 1, "Math", "floor(x)"),
    ("ceil", 1, 1, "Math", "ceil(x)"),
    ("round", 1, 1, "Math", "round(x)"),
    ("trunc", 1, 1, "Math", "trunc(x)"),
    ("frac", 1, 1, "Math", "frac(x)"),
    ("sqrt", 1, 1, "Math", "sqrt(x)"),
    ("exp", 1, 1, "Math", "exp(x)"),
    ("log", 1, 1, "Math", "log(x)"),
    ("log10", 1, 1, "Math", "log10(x)"),
    ("pow", 2, 2, "Math", "pow(x, y)"),
    ("sin", 1, 1, "Math", "sin(x)"),
    ("cos", 1, 1, "Math", "cos(x)"),
    ("tan", 1, 1, "Math", "tan(x)"),
    ("asin", 1, 1, "Math", "asin(x)"),
    ("acos", 1, 1, "Math", "acos(x)"),
    ("atan", 1, 2, "Math", "atan(x) / atan(y, x)"),
    ("atan2", 2, 2, "Math", "atan2(y, x)"),
    ("sinh", 1, 1, "Math", "sinh(x)"),
    ("cosh", 1, 1, "Math", "cosh(x)"),
    ("tanh", 1, 1, "Math", "tanh(x)"),
    ("radians", 1, 1, "Math", "radians(degrees)"),
    ("degrees", 1, 1, "Math", "degrees(radians)"),
    ("min", 1, VARIADIC, "Math", "min(a, b, ...) / min(array)"),
    ("max", 1, VARIADIC, "Math", "max(a, b, ...) / max(array)"),
    ("clamp", 3, 3, "Math", "clamp(x, lo, hi)"),
    ("mod", 2, 2, "Math", "mod(x, y)"),
    ("fit", 5, 5, "Math", "fit(x, omin, omax, nmin, nmax) (clamped)"),
    ("efit", 5, 5, "Math", "efit(x, omin, omax, nmin, nmax) (unclamped)"),
    ("fitclamp", 5, 5, "Math", "fitclamp(x, omin, omax, nmin, nmax)"),
    ("fit01", 3, 3, "Math", "fit01(x, nmin, nmax)"),
    ("fit10", 3, 3, "Math", "fit10(x, nmin, nmax)"),
    ("fit11", 3, 3, "Math", "fit11(x, nmin, nmax)"),
    ("lerp", 3, 3, "Math", "lerp(a, b, t)"),
    ("smooth", 3, 3, "Math", "smooth(edge0, edge1, x)"),
    ("smoothstep", 3, 3, "Math", "smoothstep(edge0, edge1, x)"),
    ("select", 3, 3, "Math", "select(condition, a, b)"),
    ("cbrt", 1, 1, "Math", "cbrt(x)"),
    ("invlerp", 3, 3, "Math", "invlerp(a, b, x) where x is between a and b, 0..1"),
    ("spline", 3, VARIADIC, "Math", "spline(\"linear\" | \"catrom\" | \"constant\", t, v0, v1, ...)"),
    ("isnan", 1, 1, "Math", "isnan(x)"),
    ("isfinite", 1, 1, "Math", "isfinite(x)"),
    ("ch", 1, 1, "Parameters", "ch(\"name\") a slider on the node (float)"),
    ("chf", 1, 1, "Parameters", "chf(\"name\") float slider on the node"),
    ("chi", 1, 1, "Parameters", "chi(\"name\") integer parameter on the node"),
    ("chv", 1, 1, "Parameters", "chv(\"name\") vector parameter on the node"),
    ("set", 1, 16, "Vector", "set(x, y, z) / set(x, y, z, w) / set(9 or 16 values) for matrices"),
    ("vector", 1, 3, "Vector", "vector(x) / vector(x, y, z)"),
    ("length", 1, 1, "Vector", "length(v)"),
    ("length2", 1, 1, "Vector", "length2(v)"),
    ("normalize", 1, 1, "Vector", "normalize(v)"),
    ("normalizesafe", 1, 1, "Vector", "normalizesafe(v)"),
    ("dot", 2, 2, "Vector", "dot(a, b)"),
    ("cross", 2, 2, "Vector", "cross(a, b)"),
    ("distance", 2, 2, "Vector", "distance(a, b)"),
    ("distance2", 2, 2, "Vector", "distance2(a, b)"),
    ("reflect", 2, 2, "Vector", "reflect(dir, normal)"),
    ("refract", 3, 3, "Vector", "refract(dir, normal, eta)"),
    ("avg", 1, VARIADIC, "Vector", "avg(a, b, ...) / avg(array) / avg(vector)"),
    ("luminance", 1, 1, "Vector", "luminance(color)"),
    ("hsvtorgb", 1, 1, "Vector", "hsvtorgb(hsv)"),
    ("rgbtohsv", 1, 1, "Vector", "rgbtohsv(rgb)"),
    ("vector4", 1, 4, "Matrix", "vector4(x, y, z, w) / vector4(v)"),
    ("matrix3", 1, 9, "Matrix", "matrix3(m) / matrix3(9 values) (a matrix becomes its rotation/scale part)"),
    ("matrix", 1, 16, "Matrix", "matrix(m) / matrix(16 values)"),
    ("ident", 0, 0, "Matrix", "ident() identity matrix (assign to matrix3 or matrix)"),
    ("transpose", 1, 1, "Matrix", "transpose(m)"),
    ("invert", 1, 1, "Matrix", "invert(m)"),
    ("determinant", 1, 1, "Matrix", "determinant(m)"),
    ("getcomp", 2, 3, "Matrix", "getcomp(m, row, col) / getcomp(v, i)"),
    ("setcomp", 3, 4, "Matrix", "setcomp(m, value, row, col) / setcomp(v, value, i)"),
    ("maketransform", 3, 5, "Matrix", "maketransform(t, r, s) / maketransform(trs, xyz, t, r, s); r in degrees, scale-rotate-translate"),
    ("translate", 2, 2, "Matrix", "translate(m, t) moves matrix m by t"),
    ("rotate", 3, 3, "Matrix", "rotate(m, angle, axis) rotates matrix m (radians)"),
    ("scale", 2, 2, "Matrix", "scale(m, s) scales matrix m"),
    ("lookat", 2, 3, "Matrix", "lookat(from, to [, up]) matrix3 aiming -Z at the target"),
    ("quaternion", 1, 2, "Matrix", "quaternion(angle, axis) / quaternion(matrix3) / quaternion(angle * axis)"),
    ("qrotate", 2, 2, "Matrix", "qrotate(q, v) rotates v by q"),
    ("qmultiply", 2, 2, "Matrix", "qmultiply(q1, q2) q2 applied first, then q1"),
    ("qinvert", 1, 1, "Matrix", "qinvert(q)"),
    ("qconvert", 1, 1, "Matrix", "qconvert(q) quaternion to matrix3"),
    ("slerp", 3, 3, "Matrix", "slerp(q1, q2, t)"),
    ("dihedral", 2, 2, "Matrix", "dihedral(a, b) quaternion rotating direction a onto b"),
    ("eulertoquaternion", 1, 2, "Matrix", "eulertoquaternion(radians [, order]) order 0 XYZ ... 5 ZYX"),
    ("quaterniontoeuler", 1, 2, "Matrix", "quaterniontoeuler(q [, order]) radians"),
    ("rand", 1, 2, "Random", "rand(seed [, seed2]) (vector when assigned to a vector)"),
    ("random", 1, 2, "Random", "random(seed [, seed2])"),
    ("noise", 1, 3, "Random", "noise(pos) Perlin noise in 0..1 (vector when assigned to a vector)"),
    ("snoise", 1, 3, "Random", "snoise(pos) signed Perlin noise in -1..1"),
    ("curlnoise", 1, 1, "Random", "curlnoise(pos) swirling, divergence-free noise vector"),
    ("wnoise", 1, 2, "Random", "wnoise(pos [, seed]) Worley/cellular: distance to the nearest feature point"),
    ("anoise", 1, 1, "Random", "anoise(pos) alligator (ridged, billowy) noise in 0..1"),
    ("sample_direction_uniform", 1, 1, "Random", "sample_direction_uniform(u) unit vector from u.x, u.y in 0..1"),
    ("sample_sphere_uniform", 1, 1, "Random", "sample_sphere_uniform(u) point inside the unit sphere from u in 0..1"),
    ("int", 1, 1, "Conversion", "int(x)"),
    ("float", 1, 1, "Conversion", "float(x)"),
    ("string", 1, 1, "Conversion", "string(x)"),
    ("itoa", 1, 1, "Conversion", "itoa(i)"),
    ("atoi", 1, 1, "Conversion", "atoi(s)"),
    ("atof", 1, 1, "Conversion", "atof(s)"),
    ("printf", 1, VARIADIC, "String", "printf(format, ...) prints to the node output (%d %f %g %s %v)"),
    ("sprintf", 1, VARIADIC, "String", "sprintf(format, ...)"),
    ("warning", 1, VARIADIC, "String", "warning(format, ...)"),
    ("error", 1, VARIADIC, "String", "error(format, ...) stops the cook"),
    ("concat", 1, VARIADIC, "String", "concat(a, b, ...)"),
    ("strlen", 1, 1, "String", "strlen(s)"),
    ("toupper", 1, 1, "String", "toupper(s)"),
    ("tolower", 1, 1, "String", "tolower(s)"),
    ("startswith", 2, 2, "String", "startswith(s, prefix)"),
    ("endswith", 2, 2, "String", "endswith(s, suffix)"),
    ("replace", 3, 3, "String", "replace(s, old, new)"),
    ("strip", 1, 2, "String", "strip(s [, chars]) removes whitespace (or chars) from both ends"),
    ("lstrip", 1, 2, "String", "lstrip(s [, chars])"),
    ("rstrip", 1, 2, "String", "rstrip(s [, chars])"),
    ("match", 2, 2, "String", "match(pattern, s) glob match with * and ?"),
    ("re_match", 2, 2, "String", "re_match(regex, s) 1 if s matches"),
    ("re_find", 2, 2, "String", "re_find(regex, s) the first match"),
    ("re_replace", 3, 3, "String", "re_replace(regex, replacement, s) ($1 for groups)"),
    ("re_split", 2, 2, "String", "re_split(regex, s)"),
    ("padzero", 2, 2, "String", "padzero(width, number)"),
    ("split", 1, 2, "String", "split(s [, separators])"),
    ("join", 2, 2, "String", "join(array, separator)"),
    ("array", 0, VARIADIC, "Array", "array(a, b, ...)"),
    ("len", 1, 1, "Array", "len(array | string | vector)"),
    ("push", 2, 2, "Array", "push(array, value)"),
    ("append", 2, 2, "Array", "append(array, value | array)"),
    ("pop", 1, 2, "Array", "pop(array [, index])"),
    ("insert", 3, 3, "Array", "insert(array, index, value)"),
    ("removeindex", 2, 2, "Array", "removeindex(array, index)"),
    ("removevalue", 2, 2, "Array", "removevalue(array, value)"),
    ("resize", 2, 3, "Array", "resize(array, size [, value])"),
    ("sort", 1, 1, "Array", "sort(array)"),
    ("argsort", 1, 1, "Array", "argsort(array)"),
    ("reverse", 1, 1, "Array", "reverse(array | string)"),
    ("sum", 1, 1, "Array", "sum(array)"),
    ("find", 2, 2, "Array", "find(array | string, value) index or -1"),
    ("isvalidindex", 2, 2, "Array", "isvalidindex(array, index)"),
    ("slice", 3, 4, "Array", "slice(array | string, start, end [, step]); also a[start:end:step]"),
    ("reorder", 2, 2, "Array", "reorder(array, indices) e.g. reorder(a, argsort(a))"),
    ("uniquevals", 1, 1, "Array", "uniquevals(array) sorted, without duplicates"),
    ("npoints", 1, 1, "Geometry", "npoints(0)"),
    ("nprimitives", 1, 1, "Geometry", "nprimitives(0)"),
    ("nvertices", 1, 1, "Geometry", "nvertices(0)"),
    ("point", 3, 3, "Geometry", "point(0, \"name\", ptnum) reads the input geometry"),
    ("prim", 3, 3, "Geometry", "prim(0, \"name\", primnum)"),
    ("vertex", 3, 3, "Geometry", "vertex(0, \"name\", vtxnum)"),
    ("detail", 2, 2, "Geometry", "detail(0, \"name\")"),
    ("haspointattrib", 2, 2, "Geometry", "haspointattrib(0, \"name\")"),
    ("inpointgroup", 3, 3, "Geometry", "inpointgroup(0, \"group\", ptnum)"),
    ("inprimgroup", 3, 3, "Geometry", "inprimgroup(0, \"group\", primnum)"),
    ("setpointgroup", 4, 4, "Geometry", "setpointgroup(0, \"group\", ptnum, 1 | 0)"),
    ("setprimgroup", 4, 4, "Geometry", "setprimgroup(0, \"group\", primnum, 1 | 0)"),
    ("hasprimattrib", 2, 2, "Geometry", "hasprimattrib(0, \"name\")"),
    ("setpointattrib", 4, 5, "Geometry", "setpointattrib(0, \"name\", ptnum, value [, \"set\"|\"add\"|\"min\"|\"max\"|\"mult\"])"),
    ("setprimattrib", 4, 5, "Geometry", "setprimattrib(0, \"name\", primnum, value [, mode])"),
    ("setdetailattrib", 3, 4, "Geometry", "setdetailattrib(0, \"name\", value [, mode])"),
    ("addpoint", 2, 2, "Geometry", "addpoint(0, pos | ptnum) returns the new point number"),
    ("addprim", 3, 66, "Geometry", "addprim(0, \"poly\", pt0, pt1, pt2, ...) or addprim(0, \"poly\", int points[])"),
    ("removepoint", 2, 2, "Geometry", "removepoint(0, ptnum) also removes its triangles"),
    ("removeprim", 2, 3, "Geometry", "removeprim(0, primnum [, andpoints])"),
    ("nearpoint", 2, 3, "Geometry", "nearpoint(0, pos [, maxdist]) or -1"),
    ("volumesample", 3, 3, "Volumes", "volumesample(0, \"density\", pos) a volume's (or terrain's \"height\"/\"mask\") value at pos, interpolated"),
    ("volumegradient", 3, 3, "Volumes", "volumegradient(0, \"density\", pos) direction of fastest increase, per unit"),
    ("volumeindex", 3, 3, "Volumes", "volumeindex(0, \"density\", set(ix, iy, iz)) a voxel's value (terrains: set(ix, iz, 0))"),
    ("volumepostoindex", 3, 3, "Volumes", "volumepostoindex(0, \"density\", pos) voxel index of a position"),
    ("volumeindextopos", 3, 3, "Volumes", "volumeindextopos(0, \"density\", index) position of a voxel index"),
    ("nearpoints", 3, 4, "Geometry", "nearpoints(0, pos, maxdist [, maxpts])"),
    ("neighbours", 2, 2, "Geometry", "neighbours(0, ptnum) connected points"),
    ("neighbourcount", 2, 2, "Geometry", "neighbourcount(0, ptnum)"),
    ("primpoints", 2, 2, "Geometry", "primpoints(0, primnum)"),
    ("pointprims", 2, 2, "Geometry", "pointprims(0, ptnum)"),
    ("getbbox_min", 1, 1, "Geometry", "getbbox_min(0)"),
    ("getbbox_max", 1, 1, "Geometry", "getbbox_max(0)"),
    ("getbbox_center", 1, 1, "Geometry", "getbbox_center(0)"),
    ("getbbox_size", 1, 1, "Geometry", "getbbox_size(0)"),
    ("relpointbbox", 2, 2, "Geometry", "relpointbbox(0, pos) position within the bounds, 0..1"),
    ("primuv", 4, 4, "Geometry", "primuv(0, \"name\", prim, uv) the attribute interpolated inside a primitive"),
    ("findattribval", 4, 4, "Geometry", "findattribval(0, \"point\" | \"prim\", \"name\", value) first match or -1"),
    ("idtopoint", 2, 2, "Geometry", "idtopoint(0, id) point with that @id, or -1"),
    ("primpoint", 3, 3, "Geometry", "primpoint(0, prim, vertex) point at a primitive's vertex"),
    ("primvertexcount", 2, 2, "Geometry", "primvertexcount(0, prim)"),
    ("vertexpoint", 2, 2, "Geometry", "vertexpoint(0, linearvertex)"),
    ("vertexprim", 2, 2, "Geometry", "vertexprim(0, linearvertex)"),
    ("primintrinsic", 3, 3, "Geometry", "primintrinsic(0, \"measuredarea\" | \"measuredperimeter\", prim)"),
    ("xyzdist", 2, 4, "Geometry", "xyzdist(0, pos [, prim, uv]) distance to the surface; prim and uv are outputs"),
    ("minpos", 2, 2, "Geometry", "minpos(0, pos) closest position on the surface"),
    ("intersect", 5, 5, "Geometry", "intersect(0, origin, dir, hitpos, hituv) hit prim or -1; |dir| is the max distance"),
    ("pcfind", 5, 5, "Point cloud", "pcfind(0, \"P\", pos, radius, maxpts) nearest point numbers"),
    ("pcopen", 5, 5, "Point cloud", "pcopen(0, \"P\", pos, radius, maxpts) returns a handle"),
    ("pcnumfound", 1, 1, "Point cloud", "pcnumfound(handle)"),
    ("pcfilter", 2, 2, "Point cloud", "pcfilter(handle, \"name\") distance-weighted average of an attribute"),
    ("pciterate", 1, 1, "Point cloud", "while (pciterate(handle)) { ... } visits each found point"),
    ("pcimport", 3, 3, "Point cloud", "pcimport(handle, \"name\", value) reads the current point (also \"point.number\", \"point.distance\")"),
    ("pcclose", 1, 1, "Point cloud", "pcclose(handle)"),
];

// Builtins that write results into some arguments, as VEX's pass-by-reference outputs.
const OUT_PARAMS: [(&str, &[usize]); 7] = [
    ("xyzdist", &[2, 3]),
    ("intersect", &[3, 4]),
    ("pcimport", &[2]),
    ("translate", &[0]),
    ("rotate", &[0]),
    ("scale", &[0]),
    ("setcomp", &[0]),
];

fn out_params(name: &str) -> &'static [usize] {
    OUT_PARAMS.iter().find(|(n, _)| *n == name).map_or(&[], |(_, outs)| outs)
}

// Builtins that modify their first (array) argument in place.
const MUTATING: [&str; 7] = ["push", "append", "pop", "insert", "removeindex", "removevalue", "resize"];
// Builtins whose return type follows the assignment target, as VEX overloads them.
const VECTOR_CONTEXT: [&str; 5] = ["rand", "random", "noise", "snoise", "anoise"];

fn builtin_info(name: &str) -> Option<&'static (&'static str, usize, usize, &'static str, &'static str)> {
    BUILTINS.iter().find(|b| b.0 == name)
}

// ============================================================================
// Parser
// ============================================================================

type PR<T> = Result<T, VexError>;

fn type_keyword(name: &str) -> Option<Ty> {
    match name {
        "int" => Some(Ty::Int),
        "float" => Some(Ty::Float),
        "vector" => Some(Ty::Vector),
        "string" => Some(Ty::Str),
        "vector4" => Some(Ty::Vec4),
        "matrix3" => Some(Ty::Mat3),
        "matrix" => Some(Ty::Mat4),
        _ => None,
    }
}

const UNSUPPORTED_TYPES: [&str; 5] = ["vector2", "matrix2", "dict", "bsdf", "struct"];
const QUALIFIERS: [&str; 3] = ["const", "export", "function"];

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    scopes: Vec<HashMap<String, usize>>,
    slot_types: Vec<Option<Ty>>,
    functions: Vec<Function>,
    // Every definition of each name (overloads differ in parameter types).
    fn_index: HashMap<String, Vec<usize>>,
    attrs: Vec<AttrRef>,
    attr_index: HashMap<String, usize>,
    loop_depth: usize,
    // Some(return type) while parsing a function body.
    fn_ret: Option<Option<Ty>>,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_n(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn advance(&mut self) -> Token {
        let token = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn at(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::P(q) if *q == p)
    }

    fn eat(&mut self, p: &str) -> bool {
        let found = self.at(p);
        if found {
            self.advance();
        }
        found
    }

    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(name) if name == kw)
    }

    fn err_at<T>(token: &Token, message: impl Into<String>) -> PR<T> {
        Err(VexError { line: token.line, col: token.col, message: message.into() })
    }

    fn error<T>(&self, message: impl Into<String>) -> PR<T> {
        Self::err_at(&self.toks[self.pos], message)
    }

    fn line(&self) -> usize {
        self.toks[self.pos].line
    }

    fn expect(&mut self, p: &str, context: &str) -> PR<()> {
        if self.eat(p) {
            return Ok(());
        }
        let current = &self.toks[self.pos];
        let previous = &self.toks[self.pos.saturating_sub(1)];
        // A missing ';' is reported at the end of the line it belongs to, not on the next line.
        let (line, col) = if self.pos > 0 && current.line > previous.line {
            (previous.line, previous.end_col)
        } else {
            (current.line, current.col)
        };
        Err(VexError { line, col, message: format!("expected '{p}' {context}, found {}", describe(&current.tok)) })
    }

    fn ident(&mut self, what: &str) -> PR<String> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => self.error(format!("expected {what}, found {}", describe(&other))),
        }
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name).copied())
    }

    fn declare(&mut self, name: String, ty: Option<Ty>) -> usize {
        let slot = self.slot_types.len();
        self.slot_types.push(ty);
        self.scopes.last_mut().expect("scope").insert(name, slot);
        slot
    }

    fn attr(&mut self, prefix: Option<char>, name: &str, token: &Token) -> PR<usize> {
        let hint = match prefix {
            None => known_hint(name),
            Some('f') => Some(Ty::Float),
            Some('i') => Some(Ty::Int),
            Some('v') => Some(Ty::Vector),
            Some('p') => Some(Ty::Vec4),
            Some('3') => Some(Ty::Mat3),
            Some('4') => Some(Ty::Mat4),
            Some('s') => return Self::err_at(token, "string attributes (s@) are not supported yet"),
            Some(c) => return Self::err_at(token, format!("{c}@ attributes are not supported (use f@, i@, v@, p@, 3@ or 4@)")),
        };
        if let Some(&id) = self.attr_index.get(name) {
            if prefix.is_some() {
                self.attrs[id].hint = hint;
            }
            return Ok(id);
        }
        let id = self.attrs.len();
        self.attrs.push(AttrRef { name: name.into(), kind: AttrKind::of(name), hint, written: false });
        self.attr_index.insert(name.into(), id);
        Ok(id)
    }

    fn is_decl_start(&self) -> bool {
        let mut n = 0;
        while matches!(self.peek_n(n), Tok::Ident(name) if QUALIFIERS.contains(&name.as_str())) {
            n += 1;
        }
        let Tok::Ident(name) = self.peek_n(n) else { return false };
        let is_type = type_keyword(name).is_some() || UNSUPPORTED_TYPES.contains(&name.as_str()) || name == "void";
        is_type && (n > 0 || matches!(self.peek_n(n + 1), Tok::Ident(_) | Tok::P("[")))
    }

    // Parses a type; None means void.
    fn parse_type(&mut self) -> PR<Option<Ty>> {
        while QUALIFIERS.iter().any(|q| self.at_kw(q)) {
            self.advance();
        }
        let name = self.ident("a type")?;
        if name == "void" {
            return Ok(None);
        }
        if UNSUPPORTED_TYPES.contains(&name.as_str()) {
            return self.error(format!("type '{name}' is not supported (use int, float, vector, vector4, matrix3, matrix, string, or arrays of int/float/vector/string)"));
        }
        let Some(mut ty) = type_keyword(&name) else {
            return self.error(format!("unknown type '{name}'"));
        };
        if self.at("[") && matches!(self.peek_n(1), Tok::P("]")) {
            if matches!(ty, Ty::Vec4 | Ty::Mat3 | Ty::Mat4) {
                return self.error(format!("arrays of {} aren't supported", ty.name()));
            }
            self.advance();
            self.advance();
            ty = ty.array();
        }
        Ok(Some(ty))
    }

    fn statement(&mut self, top: bool) -> PR<Option<Stmt>> {
        let line = self.line();
        let stmt = |kind| Ok(Some(Stmt { line, kind }));
        if self.eat(";") {
            return stmt(StmtKind::Block(Vec::new()));
        }
        if self.eat("{") {
            self.scopes.push(HashMap::new());
            let body = self.block_body();
            self.scopes.pop();
            return stmt(StmtKind::Block(body?));
        }
        if self.at_kw("if") {
            self.advance();
            self.expect("(", "after 'if'")?;
            let cond = self.expression()?;
            self.expect(")", "to close the if condition")?;
            let then = self.scoped_statement()?;
            let otherwise = if self.at_kw("else") {
                self.advance();
                Some(Box::new(self.scoped_statement()?))
            } else {
                None
            };
            return stmt(StmtKind::If(cond, Box::new(then), otherwise));
        }
        if self.at_kw("while") {
            self.advance();
            self.expect("(", "after 'while'")?;
            let cond = self.expression()?;
            self.expect(")", "to close the while condition")?;
            let body = self.loop_body()?;
            return stmt(StmtKind::While(cond, Box::new(body)));
        }
        if self.at_kw("do") {
            self.advance();
            let body = self.loop_body()?;
            if !self.at_kw("while") {
                return self.error("expected 'while' after the do body");
            }
            self.advance();
            self.expect("(", "after 'while'")?;
            let cond = self.expression()?;
            self.expect(")", "to close the while condition")?;
            self.expect(";", "after do ... while (...)")?;
            return stmt(StmtKind::DoWhile(Box::new(body), cond));
        }
        if self.at_kw("for") {
            self.advance();
            self.expect("(", "after 'for'")?;
            self.scopes.push(HashMap::new());
            let result = self.for_rest(line);
            self.scopes.pop();
            return result.map(Some);
        }
        if self.at_kw("foreach") {
            self.advance();
            self.expect("(", "after 'foreach'")?;
            self.scopes.push(HashMap::new());
            let result = self.foreach_rest(line);
            self.scopes.pop();
            return result.map(Some);
        }
        if self.at_kw("break") || self.at_kw("continue") {
            let is_break = self.at_kw("break");
            if self.loop_depth == 0 {
                return self.error(format!("'{}' outside a loop", if is_break { "break" } else { "continue" }));
            }
            self.advance();
            self.expect(";", "after break/continue")?;
            return stmt(if is_break { StmtKind::Break } else { StmtKind::Continue });
        }
        if self.at_kw("return") {
            self.advance();
            let value = if self.at(";") {
                None
            } else {
                let mut e = self.expression()?;
                if self.fn_ret == Some(Some(Ty::Vector)) {
                    hint_vector(&mut e);
                }
                Some(e)
            };
            self.expect(";", "after return")?;
            return stmt(StmtKind::Return(value));
        }
        if self.is_decl_start() {
            let ty = self.parse_type()?;
            let name_token = self.toks[self.pos].clone();
            let name = self.ident("a variable or function name")?;
            if self.at("(") {
                if !top || self.fn_ret.is_some() {
                    return Self::err_at(&name_token, "functions must be defined at the top level, outside other code blocks");
                }
                self.function_def(ty, name)?;
                return Ok(None);
            }
            let Some(ty) = ty else {
                return Self::err_at(&name_token, "variables cannot be void");
            };
            let decls = self.declaration_rest(ty, name, &name_token)?;
            self.expect(";", "after the declaration")?;
            return stmt(StmtKind::Block(decls));
        }
        let e = self.expression()?;
        self.expect(";", "after the statement")?;
        stmt(StmtKind::Expr(e))
    }

    fn scoped_statement(&mut self) -> PR<Stmt> {
        let line = self.line();
        self.scopes.push(HashMap::new());
        let s = self.statement(false);
        self.scopes.pop();
        Ok(s?.unwrap_or(Stmt { line, kind: StmtKind::Block(Vec::new()) }))
    }

    fn loop_body(&mut self) -> PR<Stmt> {
        self.loop_depth += 1;
        let body = self.scoped_statement();
        self.loop_depth -= 1;
        body
    }

    fn block_body(&mut self) -> PR<Vec<Stmt>> {
        let mut body = Vec::new();
        loop {
            if self.eat("}") {
                return Ok(body);
            }
            if matches!(self.peek(), Tok::Eof) {
                return self.error("missing '}' to close a block");
            }
            if let Some(s) = self.statement(false)? {
                body.push(s);
            }
        }
    }

    fn for_rest(&mut self, line: usize) -> PR<Stmt> {
        let mut init = Vec::new();
        if !self.eat(";") {
            let init_line = self.line();
            if self.is_decl_start() {
                let ty = self.parse_type()?.ok_or_else(|| VexError { line: init_line, col: 1, message: "variables cannot be void".into() })?;
                let name_token = self.toks[self.pos].clone();
                let name = self.ident("a variable name")?;
                init = self.declaration_rest(ty, name, &name_token)?;
            } else {
                init.push(Stmt { line: init_line, kind: StmtKind::Expr(self.expression()?) });
            }
            self.expect(";", "after the for-loop initializer")?;
        }
        let cond = if self.at(";") { None } else { Some(self.expression()?) };
        self.expect(";", "after the for-loop condition")?;
        let step = if self.at(")") { None } else { Some(self.expression()?) };
        self.expect(")", "to close the for-loop header")?;
        let body = self.loop_body()?;
        Ok(Stmt { line, kind: StmtKind::For(init, cond, step, Box::new(body)) })
    }

    fn foreach_rest(&mut self, line: usize) -> PR<Stmt> {
        let mut ty = self.parse_type()?.ok_or_else(|| VexError { line, col: 1, message: "foreach variables cannot be void".into() })?;
        let mut name = self.ident("the loop variable name")?;
        let mut index = None;
        self.expect(";", "after the foreach variable")?;
        if self.is_decl_start() {
            // foreach (int i; type value; array): the first variable is the index.
            if ty != Ty::Int {
                return self.error("the index variable of foreach must be an int");
            }
            index = Some(self.declare(name, Some(Ty::Int)));
            ty = self.parse_type()?.ok_or_else(|| VexError { line, col: 1, message: "foreach variables cannot be void".into() })?;
            name = self.ident("the loop variable name")?;
            self.expect(";", "after the foreach variable")?;
        }
        let array = self.expression()?;
        self.expect(")", "to close the foreach header")?;
        let var = self.declare(name, Some(ty));
        let body = self.loop_body()?;
        Ok(Stmt { line, kind: StmtKind::Foreach(index, var, array, Box::new(body)) })
    }

    fn declaration_rest(&mut self, ty: Ty, first: String, first_token: &Token) -> PR<Vec<Stmt>> {
        let mut out = Vec::new();
        let (mut name, mut token) = (first, first_token.clone());
        loop {
            let mut var_ty = ty;
            if self.eat("[") {
                if matches!(ty, Ty::Vec4 | Ty::Mat3 | Ty::Mat4) {
                    return self.error(format!("arrays of {} aren't supported", ty.name()));
                }
                if matches!(self.peek(), Tok::Int(_)) {
                    self.advance();
                }
                self.expect("]", "after '['")?;
                var_ty = ty.array();
            }
            let init = if self.eat("=") {
                let mut e = self.assignment()?;
                if var_ty == Ty::Vector {
                    hint_vector(&mut e);
                }
                Some(e)
            } else {
                None
            };
            if self.scopes.last().is_some_and(|scope| scope.contains_key(&name)) {
                return Self::err_at(&token, format!("'{name}' is already declared in this scope"));
            }
            let slot = self.declare(name, Some(var_ty));
            out.push(Stmt { line: token.line, kind: StmtKind::Decl(slot, init) });
            if !self.eat(",") {
                return Ok(out);
            }
            token = self.toks[self.pos].clone();
            name = self.ident("a variable name")?;
        }
    }

    fn function_def(&mut self, ret: Option<Ty>, name: String) -> PR<()> {
        if builtin_info(&name).is_some() {
            return self.error(format!("'{name}' is a built-in function and cannot be redefined"));
        }
        self.expect("(", "after the function name")?;
        let saved_scopes = std::mem::replace(&mut self.scopes, vec![HashMap::new()]);
        let saved_types = std::mem::take(&mut self.slot_types);
        let saved_loops = std::mem::replace(&mut self.loop_depth, 0);
        let mut param_types = Vec::new();
        if !self.eat(")") {
            let mut ty = self.param_type()?;
            loop {
                let pname = self.ident("a parameter name")?;
                let mut pty = ty;
                if self.eat("[") {
                    self.expect("]", "after '['")?;
                    pty = ty.array();
                }
                self.declare(pname, Some(pty));
                param_types.push(pty);
                if self.eat(")") {
                    break;
                }
                // VEX separates parameter groups with ';' and names of one type with ','.
                if self.eat(";") {
                    ty = self.param_type()?;
                    continue;
                }
                self.expect(",", "between parameters")?;
                if self.is_decl_start() {
                    ty = self.param_type()?;
                }
            }
        }
        // Overloads are allowed; the same parameter types twice are not.
        if self.fn_index.get(&name).is_some_and(|defs| defs.iter().any(|&i| self.functions[i].param_types == param_types)) {
            return self.error(format!("function '{name}' is already defined with these parameter types"));
        }
        let index = self.functions.len();
        self.functions.push(Function { name: name.clone(), ret, param_types, slot_types: Vec::new(), body: Vec::new() });
        self.fn_index.entry(name).or_default().push(index);
        let saved_ret = self.fn_ret.replace(ret);
        self.expect("{", "to start the function body")?;
        let body = self.block_body();
        self.fn_ret = saved_ret;
        self.functions[index].slot_types = std::mem::replace(&mut self.slot_types, saved_types);
        self.functions[index].body = body?;
        self.scopes = saved_scopes;
        self.loop_depth = saved_loops;
        Ok(())
    }

    fn param_type(&mut self) -> PR<Ty> {
        match self.parse_type()? {
            Some(ty) => Ok(ty),
            None => self.error("parameters cannot be void"),
        }
    }

    fn expression(&mut self) -> PR<Expr> {
        self.assignment()
    }

    fn assignment(&mut self) -> PR<Expr> {
        let start = self.pos;
        let lhs = self.ternary()?;
        let op = match self.peek() {
            Tok::P("=") => None,
            Tok::P("+=") => Some(BinOp::Add),
            Tok::P("-=") => Some(BinOp::Sub),
            Tok::P("*=") => Some(BinOp::Mul),
            Tok::P("/=") => Some(BinOp::Div),
            Tok::P("%=") => Some(BinOp::Mod),
            _ => return Ok(lhs),
        };
        let place = self.place_of(&lhs, start)?;
        self.advance();
        let mut rhs = self.assignment()?;
        if self.place_is_vector(&place) {
            hint_vector(&mut rhs);
        }
        Ok(Expr::Assign(Box::new(place), op, Box::new(rhs)))
    }

    fn place_of(&mut self, e: &Expr, at: usize) -> PR<Place> {
        let token = &self.toks[at];
        let Some(place) = to_place(e) else {
            return Self::err_at(token, "this cannot be assigned to (expected a variable, attribute, component or array element)");
        };
        if let Some(id) = root_attr(&place) {
            let attr = &self.attrs[id];
            if attr.kind.read_only() {
                return Self::err_at(token, format!("@{} is read-only", attr.name));
            }
            self.attrs[id].written = true;
        }
        Ok(place)
    }

    fn place_is_vector(&self, place: &Place) -> bool {
        match place {
            Place::Local(slot) => self.slot_types[*slot] == Some(Ty::Vector),
            Place::Attr(id) => self.attrs[*id].hint == Some(Ty::Vector),
            Place::Index(base, _) => matches!(**base, Place::Local(slot) if self.slot_types[slot] == Some(Ty::VecArr)),
            Place::Member(..) => false,
        }
    }

    fn ternary(&mut self) -> PR<Expr> {
        let cond = self.or()?;
        if !self.eat("?") {
            return Ok(cond);
        }
        let a = self.assignment()?;
        self.expect(":", "in the ?: expression")?;
        let b = self.ternary()?;
        Ok(Expr::Ternary(Box::new(cond), Box::new(a), Box::new(b)))
    }

    fn or(&mut self) -> PR<Expr> {
        let mut lhs = self.and()?;
        while self.eat("||") {
            lhs = Expr::Or(Box::new(lhs), Box::new(self.and()?));
        }
        Ok(lhs)
    }

    fn and(&mut self) -> PR<Expr> {
        let mut lhs = self.binary_level(0)?;
        while self.eat("&&") {
            lhs = Expr::And(Box::new(lhs), Box::new(self.binary_level(0)?));
        }
        Ok(lhs)
    }

    // Precedence levels from loosest: equality, relational, additive, multiplicative.
    fn binary_level(&mut self, level: usize) -> PR<Expr> {
        const LEVELS: [&[(&str, BinOp)]; 4] = [
            &[("==", BinOp::Eq), ("!=", BinOp::Ne)],
            &[("<", BinOp::Lt), ("<=", BinOp::Le), (">", BinOp::Gt), (">=", BinOp::Ge)],
            &[("+", BinOp::Add), ("-", BinOp::Sub)],
            &[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Mod)],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary_level(level + 1)?;
        loop {
            let Some(&(_, op)) = LEVELS[level].iter().find(|(p, _)| self.at(p)) else {
                return Ok(lhs);
            };
            self.advance();
            let rhs = self.binary_level(level + 1)?;
            lhs = match (&lhs, &rhs) {
                (Expr::Const(a), Expr::Const(b)) => match binop(op, a.clone(), b.clone()) {
                    Ok(folded) => Expr::Const(folded),
                    Err(_) => Expr::Bin(op, Box::new(lhs), Box::new(rhs)),
                },
                _ => Expr::Bin(op, Box::new(lhs), Box::new(rhs)),
            };
        }
    }

    fn unary(&mut self) -> PR<Expr> {
        if self.eat("-") {
            return Ok(match self.unary()? {
                Expr::Const(Value::Int(i)) => Expr::Const(Value::Int(i.wrapping_neg())),
                Expr::Const(Value::Float(f)) => Expr::Const(Value::Float(-f)),
                e => Expr::Neg(Box::new(e)),
            });
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.at("++") || self.at("--") {
            let delta = if self.at("++") { 1 } else { -1 };
            self.advance();
            let start = self.pos;
            let e = self.unary()?;
            let place = self.place_of(&e, start)?;
            return Ok(Expr::Step(Box::new(place), delta, true));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> PR<Expr> {
        let start = self.pos;
        let mut e = self.primary()?;
        loop {
            if self.eat(".") {
                let component = self.ident("a component name (x, y or z)")?;
                let index = match component.as_str() {
                    "x" | "r" | "u" => 0,
                    "y" | "g" | "v" => 1,
                    "z" | "b" => 2,
                    "w" | "a" => 3,
                    _ => return Self::err_at(&self.toks[self.pos - 1], format!("unknown component '.{component}' (use .x, .y, .z or .w)")),
                };
                e = Expr::Member(Box::new(e), index);
            } else if self.eat("[") {
                let start = if self.at(":") { None } else { Some(Box::new(self.expression()?)) };
                if self.eat(":") {
                    let end = if self.at(":") || self.at("]") { None } else { Some(Box::new(self.expression()?)) };
                    let step = if self.eat(":") && !self.at("]") { Some(Box::new(self.expression()?)) } else { None };
                    self.expect("]", "to close the slice")?;
                    e = Expr::Slice(Box::new(e), start, end, step);
                } else {
                    let index = start.ok_or_else(|| VexError {
                        line: self.line(),
                        col: 0,
                        message: "expected an index inside [ ]".into(),
                    })?;
                    self.expect("]", "to close the index")?;
                    e = Expr::Index(Box::new(e), index);
                }
            } else if self.at("++") || self.at("--") {
                let delta = if self.at("++") { 1 } else { -1 };
                let place = self.place_of(&e, start)?;
                self.advance();
                e = Expr::Step(Box::new(place), delta, false);
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> PR<Expr> {
        let token = self.advance();
        match token.tok.clone() {
            Tok::Int(i) => Ok(Expr::Const(Value::Int(i))),
            Tok::Float(f) => Ok(Expr::Const(Value::Float(f))),
            Tok::Str(s) => Ok(Expr::Const(Value::Str(s.into()))),
            Tok::P("(") => {
                let e = self.expression()?;
                self.expect(")", "to close the parenthesis")?;
                Ok(e)
            }
            Tok::P("{") => {
                let mut items = Vec::new();
                if !self.eat("}") {
                    loop {
                        items.push(self.assignment()?);
                        if self.eat("}") {
                            break;
                        }
                        self.expect(",", "between list items")?;
                    }
                }
                if items.iter().all(|e| matches!(e, Expr::Const(_))) {
                    let values: Vec<Value> = items.into_iter().map(|e| if let Expr::Const(v) = e { v } else { unreachable!() }).collect();
                    return Ok(Expr::Const(Value::Arr(Rc::new(values))));
                }
                Ok(Expr::List(items))
            }
            Tok::Attr(prefix, name) => Ok(Expr::Attr(self.attr(prefix, &name, &token)?)),
            Tok::Ident(name) => self.identifier(name, &token),
            other => Self::err_at(&token, format!("expected a value, found {}", describe(&other))),
        }
    }

    fn identifier(&mut self, name: String, token: &Token) -> PR<Expr> {
        if self.eat("(") {
            let mut args = Vec::new();
            if !self.eat(")") {
                loop {
                    args.push(self.assignment()?);
                    if self.eat(")") {
                        break;
                    }
                    self.expect(",", "between arguments")?;
                }
            }
            return self.call(name, args, token);
        }
        match name.as_str() {
            "M_PI" => return Ok(Expr::Const(Value::Float(std::f32::consts::PI))),
            "M_TWO_PI" => return Ok(Expr::Const(Value::Float(std::f32::consts::TAU))),
            "M_E" => return Ok(Expr::Const(Value::Float(std::f32::consts::E))),
            // Rotation and transform orders for eulertoquaternion / maketransform.
            "XFORM_XYZ" | "XFORM_SRT" => return Ok(Expr::Const(Value::Int(0))),
            "XFORM_XZY" | "XFORM_STR" => return Ok(Expr::Const(Value::Int(1))),
            "XFORM_YXZ" | "XFORM_RST" => return Ok(Expr::Const(Value::Int(2))),
            "XFORM_YZX" | "XFORM_RTS" => return Ok(Expr::Const(Value::Int(3))),
            "XFORM_ZXY" | "XFORM_TSR" => return Ok(Expr::Const(Value::Int(4))),
            "XFORM_ZYX" | "XFORM_TRS" => return Ok(Expr::Const(Value::Int(5))),
            _ => {}
        }
        if let Some(slot) = self.lookup(&name) {
            return Ok(Expr::Local(slot));
        }
        if LEGACY_ATTRS.contains(&name.as_str()) {
            return Ok(Expr::Attr(self.attr(None, &name, token)?));
        }
        if self.at("=") && type_keyword(&name).is_none() {
            // Assigning to a new name declares it, like the original wrangle syntax allowed.
            return Ok(Expr::Local(self.declare(name, None)));
        }
        Self::err_at(token, format!("unknown variable '{name}' (declare it first, e.g. float {name} = 0;)"))
    }

    fn call(&mut self, name: String, mut args: Vec<Expr>, token: &Token) -> PR<Expr> {
        if let Some(defs) = self.fn_index.get(&name).cloned() {
            let fitting: Vec<usize> = defs.iter().copied().filter(|&i| self.functions[i].param_types.len() == args.len()).collect();
            if fitting.is_empty() {
                let counts: Vec<String> = defs.iter().map(|&i| self.functions[i].param_types.len().to_string()).collect();
                return Self::err_at(token, format!("'{name}' expects {} argument(s), got {}", counts.join(" or "), args.len()));
            }
            for (k, arg) in args.iter_mut().enumerate() {
                // A {a, b, c} literal is a vector when any version takes one there.
                if fitting.iter().any(|&i| self.functions[i].param_types[k] == Ty::Vector) {
                    hint_vector(arg);
                }
            }
            let places = args.iter().map(to_place).collect();
            return Ok(match fitting.as_slice() {
                [index] => Expr::Call(*index, args, places),
                // Overloads: chosen when called, by the arguments' types.
                _ => Expr::CallOverload(fitting, args, places),
            });
        }
        let Some(&(static_name, min, max, ..)) = builtin_info(&name) else {
            return Self::err_at(token, format!("unknown function '{name}'"));
        };
        if args.len() < min || args.len() > max {
            let expected = match (min, max) {
                (a, b) if a == b => a.to_string(),
                (a, VARIADIC) => format!("at least {a}"),
                (a, b) => format!("{a} to {b}"),
            };
            return Self::err_at(token, format!("'{name}' expects {expected} argument(s), got {}", args.len()));
        }
        let mut places = Vec::new();
        let outs = out_params(static_name);
        if !outs.is_empty() {
            for (i, arg) in args.iter().enumerate() {
                let place = if outs.contains(&i) {
                    Some(to_place(arg).ok_or_else(|| VexError {
                        line: token.line,
                        col: token.col,
                        message: format!("argument {} of '{name}' is an output and must be a variable", i + 1),
                    })?)
                } else {
                    None
                };
                places.push(place);
            }
        }
        if MUTATING.contains(&static_name) {
            let place = to_place(&args[0]).ok_or_else(|| VexError {
                line: token.line,
                col: token.col,
                message: format!("the first argument of '{name}' must be an array variable"),
            })?;
            places.push(Some(place));
        }
        Ok(Expr::Builtin(static_name, args, places, false))
    }
}

// Marks rand/noise calls whose result is assigned to a vector so they return vectors.
fn hint_vector(e: &mut Expr) {
    match e {
        Expr::Builtin(name, _, _, want) if VECTOR_CONTEXT.contains(name) => *want = true,
        Expr::Bin(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div, a, b) => {
            hint_vector(a);
            hint_vector(b);
        }
        Expr::Neg(a) => hint_vector(a),
        Expr::Ternary(_, a, b) => {
            hint_vector(a);
            hint_vector(b);
        }
        _ => {}
    }
}

// The ch() parameters `code` uses, in order of first use: (name, kind) with kind
// 'f' (ch/chf), 'i' (chi) or 'v' (chv).
pub fn channel_refs(code: &str) -> Vec<(String, char)> {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| regex::Regex::new(r#"\bch([fiv]?)\s*\(\s*"([^"]+)"\s*\)"#).unwrap());
    let mut out: Vec<(String, char)> = Vec::new();
    for cap in pattern.captures_iter(code) {
        let kind = cap[1].chars().next().unwrap_or('f');
        if !out.iter().any(|(n, _)| n == &cap[2]) {
            out.push((cap[2].to_owned(), kind));
        }
    }
    out
}

pub fn compile(src: &str) -> Result<Program, VexError> {
    let mut parser = Parser {
        toks: lex(src)?,
        pos: 0,
        scopes: vec![HashMap::new()],
        slot_types: Vec::new(),
        functions: Vec::new(),
        fn_index: HashMap::new(),
        attrs: Vec::new(),
        attr_index: HashMap::new(),
        loop_depth: 0,
        fn_ret: None,
    };
    let mut body = Vec::new();
    while !matches!(parser.peek(), Tok::Eof) {
        if let Some(stmt) = parser.statement(true)? {
            body.push(stmt);
        }
    }
    Ok(Program {
        main: Function { name: "main".into(), ret: None, param_types: Vec::new(), slot_types: parser.slot_types, body },
        functions: parser.functions,
        attrs: parser.attrs,
    })
}

// ============================================================================
// Interpreter
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
enum Domain {
    Detail,
    Prim,
    Point,
    Vertex,
}

impl Domain {
    fn prefix(self) -> &'static str {
        match self {
            Domain::Detail => "detail:",
            Domain::Prim => "prim:",
            Domain::Point => "",
            Domain::Vertex => "vertex:",
        }
    }

    fn of_key(key: &str) -> Domain {
        if key.starts_with("prim:") {
            Domain::Prim
        } else if key.starts_with("vertex:") {
            Domain::Vertex
        } else if key.starts_with("detail:") {
            Domain::Detail
        } else {
            Domain::Point
        }
    }

    fn count(self, mesh: &Mesh) -> usize {
        match self {
            Domain::Detail => 1,
            Domain::Prim => mesh.indices.len(),
            Domain::Point => mesh.vertices.len(),
            Domain::Vertex => mesh.indices.len() * 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum WriteMode {
    Set,
    Add,
    Min,
    Max,
    Mult,
}

#[derive(Default)]
struct Pending {
    add_points: Vec<[f32; 3]>,
    // Polygons added by addprim (their points, in order).
    add_prims: Vec<Vec<usize>>,
    remove_points: HashSet<usize>,
    remove_prims: HashSet<usize>,
    // Points of removed primitives to delete if nothing else uses them (removeprim andpoints).
    orphan_candidates: HashSet<usize>,
    writes: Vec<(Domain, String, usize, Value, WriteMode)>,
}

enum Flow {
    Normal,
    Break,
    Continue,
    Return(Value),
}

struct Frame<'a> {
    vals: Vec<Value>,
    types: &'a [Option<Ty>],
}

struct Runtime<'a> {
    prog: &'a Program,
    input: &'a Mesh,
    normals: &'a [Vec3],
    out: Mesh,
    domain: Domain,
    count: usize,
    // Storage key (domain prefix + name) of every attribute the program uses.
    keys: Vec<String>,
    elem: usize,
    time: f32,
    // Seconds per step: one frame, or one SOP Solver substep.
    time_inc: f32,
    pending: Pending,
    steps: u64,
    depth: usize,
    log: Vec<String>,
    partial_line: String,
    neighbours: Option<Vec<Vec<usize>>>,
    point_prims: Option<Vec<Vec<usize>>>,
    point_grid: Option<PointGrid>,
    // Fields read with volumesample() and friends, by name.
    fields: Vec<(String, crate::fields::Field)>,
    // Values of the node's ch() parameters.
    channels: Option<&'a Channels>,
    // pcopen handles for the current element (cleared when the element changes).
    pc_handles: Vec<PcHandle>,
    pc_elem: usize,
    // Primitives are polygons: the input's polygons (their triangles), each
    // triangle's polygon, and the polygon being run over (`elem` is its first triangle).
    polys: Vec<std::ops::Range<usize>>,
    poly_of: Vec<usize>,
    prim: usize,
}

// A polygon's uv names a point on one of its triangles: u spans the triangles
// in turn (u·n = triangle + its barycentric u), v is that triangle's v. A
// single triangle's uv is its own barycentric uv.
fn poly_uv_to_triangle(run: &std::ops::Range<usize>, uv: [f32; 3]) -> (usize, [f32; 2]) {
    let n = run.len().max(1);
    let scaled = uv[0].max(0.0) * n as f32;
    let k = (scaled.floor() as usize).min(n - 1);
    (run.start + k, [scaled - k as f32, uv[1]])
}

fn vec3(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[1], v[2])
}

fn arr3(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

fn with_line(error: String, line: usize) -> String {
    if error.starts_with("line ") {
        error
    } else {
        format!("line {line}: {error}")
    }
}

// Reads one element of a stored attribute; missing attributes read as zero of the hinted type.
fn read_storage(mesh: &Mesh, key: &str, index: usize, count: usize, hint: Option<Ty>) -> Value {
    match mesh.point_attributes.get(key) {
        Some(vals) if count > 0 && !vals.is_empty() => {
            let comps = (vals.len() / count).max(1);
            let at = |c: usize| vals.get(index * comps + c).copied().unwrap_or(0.0);
            // Wide attributes: the width (or the binding's type) decides.
            match (hint, comps) {
                (Some(Ty::Vec4), _) | (None, 4) => return Value::Vec4(std::array::from_fn(|c| if c < comps { at(c) } else if c == 3 { 1.0 } else { 0.0 })),
                (Some(Ty::Mat3), _) | (None, 9) if comps >= 9 => return Value::Mat3(std::array::from_fn(at)),
                (Some(Ty::Mat4), _) | (None, 16) if comps >= 16 => return Value::Mat4(std::array::from_fn(at)),
                _ => {}
            }
            if comps >= 3 {
                Value::Vec([at(0), at(1), at(2)])
            } else if hint == Some(Ty::Int) {
                Value::Int(at(0).round() as i32)
            } else {
                Value::Float(at(0))
            }
        }
        _ => hint.filter(|t| *t != Ty::Str).map_or(Value::Float(0.0), Ty::default_value),
    }
}

// Re-lays out an attribute array with a different number of components per element.
fn reshape(old: &[f32], count: usize, comps: usize) -> Vec<f32> {
    let old_comps = if count > 0 { old.len() / count } else { 0 };
    let mut out = vec![0.0; count * comps];
    for i in 0..count {
        for c in 0..comps {
            out[i * comps + c] = match old_comps {
                0 => 0.0,
                1 => old[i],
                n if c < n => old[i * n + c],
                _ => 0.0,
            };
        }
    }
    out
}

fn write_storage(mesh: &mut Mesh, key: &str, index: usize, count: usize, v: &Value, hint: Option<Ty>, name: &str) -> R<()> {
    match v {
        Value::Str(_) => return Err(format!("cannot store a string in @{name}: string attributes are not supported yet")),
        // {..} literals for wide attributes (p@orient = {0, 0, 0, 1}) have that many items.
        Value::Arr(items)
            if items.len() != 3 && !matches!((hint, items.len()), (Some(Ty::Vec4), 4) | (Some(Ty::Mat3), 9) | (Some(Ty::Mat4), 16)) =>
        {
            return Err(format!("cannot store an array in @{name}: array attributes are not supported"))
        }
        _ => {}
    }
    if index >= count {
        return Ok(());
    }
    let existing = mesh
        .point_attributes
        .get(key)
        .filter(|vals| count > 0 && !vals.is_empty())
        .map(|vals| (vals.len() / count).max(1));
    let wide = v.floats().map(<[f32]>::len).or(match hint {
        Some(Ty::Vec4) => Some(4),
        Some(Ty::Mat3) => Some(9),
        Some(Ty::Mat4) => Some(16),
        _ => None,
    });
    let comps = if let Some(width) = wide {
        width
    } else if v.is_vector_like() || hint == Some(Ty::Vector) || name == "Cd" {
        existing.filter(|c| *c >= 3).unwrap_or(3)
    } else {
        existing.unwrap_or(1)
    };
    let vals = mesh.point_attributes.entry(key.to_string()).or_default();
    if vals.len() != count * comps {
        *vals = reshape(vals, count, comps);
    }
    let floats: Vec<f32> = match comps {
        4 => v.vec4()?.to_vec(),
        9 => v.mat3()?.to_vec(),
        16 => v.mat4()?.to_vec(),
        1 => vec![v.num()?],
        _ => v.vec()?.to_vec(),
    };
    vals[index * comps..index * comps + floats.len().min(comps)].copy_from_slice(&floats[..floats.len().min(comps)]);
    Ok(())
}

fn tri_centroid(mesh: &Mesh, tri: [usize; 3]) -> Vec3 {
    mesh.vertices[tri[0]].add(mesh.vertices[tri[1]]).add(mesh.vertices[tri[2]]).scale(1.0 / 3.0)
}

fn tri_normal(mesh: &Mesh, tri: [usize; 3]) -> Vec3 {
    let [a, b, c] = tri.map(|i| mesh.vertices[i]);
    b.sub(a).cross(c.sub(a)).normalize()
}

// Point normal: an explicit N attribute when present, else the computed normal.
fn point_normal(mesh: &Mesh, normals: &[Vec3], pt: usize) -> Value {
    if let Some(vals) = mesh.point_attributes.get("N").filter(|vals| vals.len() == mesh.vertices.len() * 3) {
        return Value::Vec([vals[pt * 3], vals[pt * 3 + 1], vals[pt * 3 + 2]]);
    }
    Value::Vec(arr3(normals.get(pt).copied().unwrap_or(Vec3::new(0.0, 1.0, 0.0))))
}

fn fit(x: f32, omin: f32, omax: f32, nmin: f32, nmax: f32, clamp: bool) -> f32 {
    let range = omax - omin;
    let mut t = if range.abs() < 1e-12 { 0.0 } else { (x - omin) / range };
    if clamp {
        t = t.clamp(0.0, 1.0);
    }
    nmin + t * (nmax - nmin)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = if (edge1 - edge0).abs() < 1e-12 { if x < edge0 { 0.0 } else { 1.0 } } else { ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0) };
    t * t * (3.0 - 2.0 * t)
}

// Applies `f` per component when any argument is a vector, otherwise once on scalars.
fn per_lane(args: &[Value], f: impl Fn(&[f32]) -> f32) -> R<Value> {
    let mut lanes = [0.0f32; 8];
    let n = args.len().min(8);
    if args.iter().any(Value::is_vector_like) {
        let mut vs = [[0.0f32; 3]; 8];
        for (i, a) in args.iter().take(8).enumerate() {
            vs[i] = a.vec()?;
        }
        let mut out = [0.0; 3];
        for (lane, slot) in out.iter_mut().enumerate() {
            for i in 0..n {
                lanes[i] = vs[i][lane];
            }
            *slot = f(&lanes[..n]);
        }
        return Ok(Value::Vec(out));
    }
    for (i, a) in args.iter().take(8).enumerate() {
        lanes[i] = a.num()?;
    }
    Ok(Value::Float(f(&lanes[..n])))
}

fn all_int(args: &[Value]) -> bool {
    args.iter().all(|v| matches!(v, Value::Int(_)))
}

fn pick_extreme(a: Value, b: Value, min: bool) -> R<Value> {
    if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
        return Ok(Value::Int(if min { *x.min(y) } else { *x.max(y) }));
    }
    per_lane(&[a, b], |x| if min { x[0].min(x[1]) } else { x[0].max(x[1]) })
}

fn average(items: &[Value]) -> R<Value> {
    if items.is_empty() {
        return Ok(Value::Float(0.0));
    }
    let n = items.len() as f32;
    if items.iter().any(Value::is_vector_like) {
        let mut sum = [0.0f32; 3];
        for item in items {
            let v = item.vec()?;
            (0..3).for_each(|c| sum[c] += v[c]);
        }
        return Ok(Value::Vec(sum.map(|c| c / n)));
    }
    Ok(Value::Float(items.iter().map(Value::num).sum::<R<f32>>()? / n))
}

fn seed_bits(v: &Value) -> R<u32> {
    Ok(match v {
        Value::Vec(_) | Value::Arr(_) => {
            let p = v.vec()?;
            p[0].to_bits() ^ p[1].to_bits().rotate_left(11) ^ p[2].to_bits().rotate_left(22)
        }
        other => other.num()?.to_bits(),
    })
}

fn lattice_hash(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD816_3841)
        ^ (z as u32).wrapping_mul(0xCB1A_B31F)
        ^ seed.wrapping_mul(0x1656_67B1);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5BD1_E995);
    h ^ (h >> 15)
}

fn gradient(hash: u32, x: f32, y: f32, z: f32) -> f32 {
    let h = hash & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 { y } else if h == 12 || h == 14 { x } else { z };
    (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
}

// Improved Perlin gradient noise, roughly in -1..1.
pub(crate) fn perlin3(p: [f32; 3], seed: u32) -> f32 {
    let cell = p.map(f32::floor);
    let [ix, iy, iz] = cell.map(|c| c as i32);
    let [fx, fy, fz] = [p[0] - cell[0], p[1] - cell[1], p[2] - cell[2]];
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v, w) = (fade(fx), fade(fy), fade(fz));
    let g = |dx: i32, dy: i32, dz: i32| {
        gradient(lattice_hash(ix + dx, iy + dy, iz + dz, seed), fx - dx as f32, fy - dy as f32, fz - dz as f32)
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    lerp(
        lerp(lerp(g(0, 0, 0), g(1, 0, 0), u), lerp(g(0, 1, 0), g(1, 1, 0), u), v),
        lerp(lerp(g(0, 0, 1), g(1, 0, 1), u), lerp(g(0, 1, 1), g(1, 1, 1), u), v),
        w,
    )
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h = (h - h.floor()) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    let h = if delta <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    [h, if max > 0.0 { delta / max } else { 0.0 }, max]
}

// printf-style formatting: %d %i %f %g %e %s %c %x %v with flags, width and precision.
fn format_vex(args: &[Value]) -> R<String> {
    let fmt = args[0].string()?;
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut next_arg = 1;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        if chars.get(i) == Some(&'%') {
            out.push('%');
            i += 1;
            continue;
        }
        let (mut left, mut zero, mut plus) = (false, false, false);
        while let Some(&flag) = chars.get(i) {
            match flag {
                '-' => left = true,
                '0' => zero = true,
                '+' => plus = true,
                ' ' => {}
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        while let Some(d) = chars.get(i).and_then(|c| c.to_digit(10)) {
            width = width * 10 + d as usize;
            i += 1;
        }
        let mut precision = None;
        if chars.get(i) == Some(&'.') {
            i += 1;
            let mut p = 0usize;
            while let Some(d) = chars.get(i).and_then(|c| c.to_digit(10)) {
                p = p * 10 + d as usize;
                i += 1;
            }
            precision = Some(p);
        }
        while chars.get(i) == Some(&'l') {
            i += 1;
        }
        let Some(&conv) = chars.get(i) else {
            out.push('%');
            break;
        };
        i += 1;
        let arg = args.get(next_arg).ok_or_else(|| format!("not enough arguments for the format \"{fmt}\""))?;
        next_arg += 1;
        let body = format_one(conv, arg, precision, plus)?;
        let pad = width.saturating_sub(body.chars().count());
        if left {
            out.push_str(&body);
            out.extend(std::iter::repeat(' ').take(pad));
        } else if zero && !matches!(conv, 's' | 'c' | 'v') && !arg.is_vector_like() {
            let (sign, digits) = if body.starts_with('-') || body.starts_with('+') { body.split_at(1) } else { ("", body.as_str()) };
            out.push_str(sign);
            out.extend(std::iter::repeat('0').take(pad));
            out.push_str(digits);
        } else {
            out.extend(std::iter::repeat(' ').take(pad));
            out.push_str(&body);
        }
    }
    Ok(out)
}

fn format_one(conv: char, v: &Value, precision: Option<usize>, plus: bool) -> R<String> {
    if let Value::Vec(c) = v {
        let parts = c.iter().map(|x| format_one(conv, &Value::Float(*x), precision, plus)).collect::<R<Vec<_>>>()?;
        return Ok(format!("{{{}}}", parts.join(", ")));
    }
    if let Value::Arr(items) = v {
        let parts = items.iter().map(|x| format_one(conv, x, precision, plus)).collect::<R<Vec<_>>>()?;
        return Ok(format!("{{{}}}", parts.join(", ")));
    }
    let sign = |s: String, positive: bool| if plus && positive { format!("+{s}") } else { s };
    Ok(match conv {
        'd' | 'i' => {
            let n = v.int()?;
            sign(n.to_string(), n >= 0)
        }
        'f' | 'F' => {
            let f = v.num()?;
            sign(format!("{:.*}", precision.unwrap_or(6), f), f >= 0.0)
        }
        'e' | 'E' => {
            let f = v.num()?;
            let s = format!("{:.*e}", precision.unwrap_or(6), f);
            sign(if conv == 'E' { s.to_uppercase() } else { s }, f >= 0.0)
        }
        'g' | 'G' => match v {
            Value::Str(s) => s.to_string(),
            _ => {
                let f = v.num()?;
                let s = match precision {
                    Some(p) => {
                        let s = format!("{:.*}", p, f);
                        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
                    }
                    None => f.to_string(),
                };
                sign(s, f >= 0.0)
            }
        },
        's' | 'v' => match (v, precision) {
            (Value::Float(f), Some(p)) => format!("{:.*}", p, f),
            _ => v.display(),
        },
        'c' => char::from_u32(v.int()? as u32).map(String::from).unwrap_or_default(),
        'x' => format!("{:x}", v.int()?),
        'X' => format!("{:X}", v.int()?),
        other => return Err(format!("unknown format '%{other}'")),
    })
}

fn sort_values(items: &[Value]) -> R<Vec<usize>> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    if items.iter().all(|v| matches!(v, Value::Str(_))) {
        order.sort_by(|&a, &b| items[a].display().cmp(&items[b].display()));
    } else {
        let keys = items.iter().map(Value::num).collect::<R<Vec<f32>>>().map_err(|_| "can only sort numbers or strings".to_string())?;
        order.sort_by(|&a, &b| keys[a].partial_cmp(&keys[b]).unwrap_or(std::cmp::Ordering::Equal));
    }
    Ok(order)
}

// Worley (cellular) noise: distance to the nearest of one random feature point per unit cell.
fn worley(p: [f32; 3], seed: u32) -> f32 {
    let cell = p.map(|c| c.floor() as i32);
    let mut best = f32::INFINITY;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                let h = (c[0] as u32).wrapping_mul(73_856_093) ^ (c[1] as u32).wrapping_mul(19_349_663) ^ (c[2] as u32).wrapping_mul(83_492_791);
                let feature = [0u32, 1, 2].map(|k| hash01(h.wrapping_add(k * 0x9E37), seed));
                let d: f32 = (0..3).map(|k| (c[k] as f32 + feature[k] - p[k]).powi(2)).sum::<f32>().sqrt();
                best = best.min(d);
            }
        }
    }
    best
}

// Glob matching with * (any run) and ? (one character).
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

fn cached_regex(pattern: &str) -> R<regex::Regex> {
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<String, regex::Regex>> = std::cell::RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(re) = cache.get(pattern) {
            return Ok(re.clone());
        }
        let re = regex::Regex::new(pattern).map_err(|e| format!("bad regular expression: {e}"))?;
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(pattern.to_owned(), re.clone());
        Ok(re)
    })
}

// a[start:end:step] with Python semantics (negative indices count from the end).
fn slice_value(base: &Value, start: Option<i32>, end: Option<i32>, step: i32) -> R<Value> {
    if step == 0 {
        return Err("slice step cannot be 0".into());
    }
    let pick = |len: usize| -> Vec<usize> {
        let len = len as i64;
        let clamp = |i: Option<i32>, default: i64, lo: i64, hi: i64| match i {
            Some(i) => {
                let i = i as i64;
                (if i < 0 { i + len } else { i }).clamp(lo, hi)
            }
            None => default,
        };
        let mut out = Vec::new();
        if step > 0 {
            let (mut i, end) = (clamp(start, 0, 0, len), clamp(end, len, 0, len));
            while i < end {
                out.push(i as usize);
                i += step as i64;
            }
        } else {
            let (mut i, end) = (clamp(start, len - 1, -1, len - 1), clamp(end, -1, -1, len - 1));
            while i > end {
                out.push(i as usize);
                i += step as i64;
            }
        }
        out
    };
    match base {
        Value::Arr(items) => Ok(Value::Arr(Rc::new(pick(items.len()).into_iter().map(|i| items[i].clone()).collect()))),
        Value::Str(s) => {
            let chars: Vec<char> = s.chars().collect();
            Ok(Value::Str(pick(chars.len()).into_iter().map(|i| chars[i]).collect::<String>().into()))
        }
        other => Err(format!("cannot slice a {}", other.type_name())),
    }
}

// Uniform hash grid over the input points, for nearpoint(s) and pcfind/pcopen.
struct PointGrid {
    cell: f32,
    cells: HashMap<[i32; 3], Vec<usize>>,
    lo: [i32; 3],
    hi: [i32; 3],
}

impl PointGrid {
    fn new(points: &[Vec3]) -> Self {
        let mut grid = PointGrid { cell: 1.0, cells: HashMap::new(), lo: [0; 3], hi: [0; 3] };
        let Some(first) = points.first() else { return grid };
        let (mut lo, mut hi) = (*first, *first);
        for p in points {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        let extent = hi.sub(lo);
        // About one point per occupied cell for evenly spread points.
        grid.cell = (extent.x.max(extent.y).max(extent.z) / (points.len() as f32).cbrt()).max(1e-5);
        grid.lo = grid.key(lo);
        grid.hi = grid.key(hi);
        for (i, p) in points.iter().enumerate() {
            grid.cells.entry(grid.key(*p)).or_default().push(i);
        }
        grid
    }

    fn key(&self, p: Vec3) -> [i32; 3] {
        [p.x, p.y, p.z].map(|c| (c / self.cell).floor().clamp(-1e9, 1e9) as i32)
    }

    fn brute(points: &[Vec3], p: Vec3, radius: f32) -> Vec<(usize, f32)> {
        points.iter().enumerate().map(|(i, v)| (i, v.sub(p).length())).filter(|(_, d)| *d <= radius).collect()
    }

    // Points within `radius` of `p`, nearest first, at most `limit`.
    fn within(&self, points: &[Vec3], p: Vec3, radius: f32, limit: usize) -> Vec<(usize, f32)> {
        let mut found = if !radius.is_finite() {
            Self::brute(points, p, radius)
        } else {
            let (a, b) = (self.key(p.sub(Vec3::new(radius, radius, radius))), self.key(p.add(Vec3::new(radius, radius, radius))));
            let (a, b) = ([0, 1, 2].map(|i| a[i].max(self.lo[i])), [0, 1, 2].map(|i| b[i].min(self.hi[i])));
            let span: i64 = (0..3).map(|i| (b[i] as i64 - a[i] as i64 + 1).max(0)).product();
            if span > self.cells.len() as i64 * 2 {
                Self::brute(points, p, radius)
            } else {
                let mut found = Vec::new();
                for z in a[2]..=b[2] {
                    for y in a[1]..=b[1] {
                        for x in a[0]..=b[0] {
                            for &i in self.cells.get(&[x, y, z]).map_or(&[][..], Vec::as_slice) {
                                let d = points[i].sub(p).length();
                                if d <= radius {
                                    found.push((i, d));
                                }
                            }
                        }
                    }
                }
                found
            }
        };
        found.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal).then(x.0.cmp(&y.0)));
        found.truncate(limit);
        found
    }

    // Nearest point within `max`, searching rings of cells outward from `p`.
    fn nearest(&self, points: &[Vec3], p: Vec3, max: f32) -> Option<(usize, f32)> {
        if points.is_empty() {
            return None;
        }
        let c = self.key(p);
        let rings = (0..3).map(|i| (c[i] - self.lo[i]).abs().max((c[i] - self.hi[i]).abs())).max().unwrap_or(0);
        let mut best: Option<(usize, f32)> = None;
        for k in 0..=rings {
            if (2 * k as i64 + 1).pow(3) > points.len() as i64 * 8 {
                // Very sparse or far away: a direct scan is cheaper.
                return Self::brute(points, p, max)
                    .into_iter()
                    .min_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal).then(x.0.cmp(&y.0)));
            }
            for z in c[2] - k..=c[2] + k {
                for y in c[1] - k..=c[1] + k {
                    for x in c[0] - k..=c[0] + k {
                        if (x - c[0]).abs() != k && (y - c[1]).abs() != k && (z - c[2]).abs() != k {
                            continue;
                        }
                        for &i in self.cells.get(&[x, y, z]).map_or(&[][..], Vec::as_slice) {
                            let d = points[i].sub(p).length();
                            if d <= max && best.is_none_or(|(bi, bd)| d < bd || (d == bd && i < bi)) {
                                best = Some((i, d));
                            }
                        }
                    }
                }
            }
            // Anything in a later ring is at least k cells away.
            if best.is_some_and(|(_, d)| d <= k as f32 * self.cell) || k as f32 * self.cell > max {
                break;
            }
        }
        best
    }
}

struct PcHandle {
    found: Vec<(usize, f32)>,
    radius: f32,
    next: usize,
    current: Option<usize>,
}

struct SurfaceHit {
    prim: usize,
    position: Vec3,
    // Parametric coordinates: position = (1-u-v)*p0 + u*p1 + v*p2.
    uv: [f32; 2],
}

fn barycentric_uv(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 2] {
    let (e1, e2, d) = (b.sub(a), c.sub(a), p.sub(a));
    let (d11, d12, d22) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
    let (d1, d2) = (d.dot(e1), d.dot(e2));
    let denom = d11 * d22 - d12 * d12;
    if denom.abs() < 1e-20 {
        return [0.0, 0.0];
    }
    [(d22 * d1 - d12 * d2) / denom, (d11 * d2 - d12 * d1) / denom]
}

// Closest point on any triangle of `mesh`; points only, the nearest point.
fn closest_on_surface(mesh: &Mesh, p: Vec3) -> Option<SurfaceHit> {
    if mesh.indices.is_empty() {
        let (i, _) = mesh
            .vertices
            .iter()
            .enumerate()
            .map(|(i, v)| (i, v.sub(p).length()))
            .min_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal))?;
        // No primitives: the nearest point, with no prim to report.
        return Some(SurfaceHit { prim: usize::MAX, position: mesh.vertices[i], uv: [0.0, 0.0] });
    }
    let mut best: Option<(f32, SurfaceHit)> = None;
    for (prim, tri) in mesh.indices.iter().enumerate() {
        let [a, b, c] = tri.map(|i| mesh.vertices[i]);
        let q = crate::volume::closest_on_triangle(p, a, b, c);
        let d = q.sub(p).length();
        if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = Some((d, SurfaceHit { prim, position: q, uv: barycentric_uv(q, a, b, c) }));
        }
    }
    best.map(|(_, hit)| hit)
}

// First triangle hit by the segment origin..origin+dir (Moller-Trumbore).
fn ray_hit(mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<SurfaceHit> {
    let mut best: Option<(f32, SurfaceHit)> = None;
    for (prim, tri) in mesh.indices.iter().enumerate() {
        let [a, b, c] = tri.map(|i| mesh.vertices[i]);
        let (e1, e2) = (b.sub(a), c.sub(a));
        let h = dir.cross(e2);
        let det = e1.dot(h);
        if det.abs() < 1e-12 {
            continue;
        }
        let s = origin.sub(a);
        let u = s.dot(h) / det;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = s.cross(e1);
        let v = dir.dot(q) / det;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = e2.dot(q) / det;
        if (0.0..=1.0).contains(&t) && best.as_ref().is_none_or(|(bt, _)| t < *bt) {
            best = Some((t, SurfaceHit { prim, position: origin.add(dir.scale(t)), uv: [u, v] }));
        }
    }
    best.map(|(_, hit)| hit)
}

fn int_array(values: impl IntoIterator<Item = usize>) -> Value {
    Value::Arr(Rc::new(values.into_iter().map(|i| Value::Int(i as i32)).collect()))
}

fn write_mode(v: Option<&Value>) -> R<WriteMode> {
    let Some(v) = v else { return Ok(WriteMode::Set) };
    Ok(match &*v.string()? {
        "set" => WriteMode::Set,
        "add" => WriteMode::Add,
        "min" => WriteMode::Min,
        "max" => WriteMode::Max,
        "mult" | "multiply" => WriteMode::Mult,
        other => return Err(format!("unknown write mode \"{other}\" (use set, add, min, max or mult)")),
    })
}

fn mutate_array(name: &str, target: Value, args: &[Value]) -> R<(Value, Value)> {
    let mut items = match target {
        Value::Arr(items) => Rc::try_unwrap(items).unwrap_or_else(|shared| (*shared).clone()),
        other => return Err(format!("the first argument must be an array, got {}", other.type_name())),
    };
    let result = match name {
        "push" => {
            items.push(args[0].clone());
            Value::Int(items.len() as i32)
        }
        "append" => {
            match &args[0] {
                Value::Arr(more) => items.extend(more.iter().cloned()),
                other => items.push(other.clone()),
            }
            Value::Int(items.len() as i32)
        }
        "pop" => {
            let index = match args.first() {
                Some(i) => normalize_index(i.int()?, items.len()),
                None => items.len().checked_sub(1),
            };
            match index.filter(|i| *i < items.len()) {
                Some(i) => items.remove(i),
                None => Value::Int(0),
            }
        }
        "insert" => {
            let index = normalize_index(args[0].int()?, items.len()).unwrap_or(0).min(items.len());
            items.insert(index, args[1].clone());
            Value::Int(items.len() as i32)
        }
        "removeindex" => match normalize_index(args[0].int()?, items.len()).filter(|i| *i < items.len()) {
            Some(i) => items.remove(i),
            None => Value::Int(0),
        },
        "removevalue" => match items.iter().position(|v| values_equal(v, &args[0])) {
            Some(i) => {
                items.remove(i);
                Value::Int(1)
            }
            None => Value::Int(0),
        },
        "resize" => {
            let size = args[0].int()?.max(0) as usize;
            let value = args.get(1).cloned().unwrap_or_else(|| items.first().map_or(Value::Int(0), Value::zero_like));
            items.resize(size, value);
            Value::Int(size as i32)
        }
        _ => return Err(format!("internal: {name} is not an array function")),
    };
    Ok((Value::Arr(Rc::new(items)), result))
}

impl<'a> Runtime<'a> {
    fn tick(&mut self) -> R<()> {
        self.steps += 1;
        if self.steps > STEP_LIMIT {
            return Err(format!("stopped after {STEP_LIMIT} loop iterations: is there an infinite loop?"));
        }
        Ok(())
    }

    fn point_index(&self) -> Option<usize> {
        match self.domain {
            Domain::Point => Some(self.elem),
            Domain::Vertex => Some(self.out.indices[self.elem / 3][self.elem % 3]),
            _ => None,
        }
    }

    fn print(&mut self, text: &str) {
        self.partial_line.push_str(text);
        while let Some(newline) = self.partial_line.find('\n') {
            let line: String = self.partial_line.drain(..=newline).collect();
            self.push_log(line.trim_end_matches('\n').to_string());
        }
    }

    fn push_log(&mut self, line: String) {
        if self.log.len() < MAX_LOG_LINES {
            self.log.push(line);
        } else if self.log.len() == MAX_LOG_LINES {
            self.log.push(format!("… output truncated after {MAX_LOG_LINES} lines"));
        }
    }

    fn exec_block(&mut self, stmts: &'a [Stmt], fr: &mut Frame<'a>) -> R<Flow> {
        for s in stmts {
            match self.exec(s, fr)? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec(&mut self, s: &'a Stmt, fr: &mut Frame<'a>) -> R<Flow> {
        self.exec_kind(&s.kind, fr).map_err(|e| with_line(e, s.line))
    }

    fn exec_kind(&mut self, kind: &'a StmtKind, fr: &mut Frame<'a>) -> R<Flow> {
        match kind {
            StmtKind::Expr(e) => {
                self.eval(e, fr)?;
            }
            StmtKind::Decl(slot, init) => {
                let value = match init {
                    Some(e) => self.eval(e, fr)?,
                    None => fr.types[*slot].map_or(Value::Int(0), Ty::default_value),
                };
                store_local(fr, *slot, value)?;
            }
            StmtKind::If(cond, then, otherwise) => {
                if self.eval(cond, fr)?.truthy() {
                    return self.exec(then, fr);
                } else if let Some(otherwise) = otherwise {
                    return self.exec(otherwise, fr);
                }
            }
            StmtKind::While(cond, body) => loop {
                self.tick()?;
                if !self.eval(cond, fr)?.truthy() {
                    break;
                }
                match self.exec(body, fr)? {
                    Flow::Break => break,
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                    _ => {}
                }
            },
            StmtKind::DoWhile(body, cond) => loop {
                self.tick()?;
                match self.exec(body, fr)? {
                    Flow::Break => break,
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                    _ => {}
                }
                if !self.eval(cond, fr)?.truthy() {
                    break;
                }
            },
            StmtKind::For(init, cond, step, body) => {
                for s in init {
                    self.exec(s, fr)?;
                }
                loop {
                    self.tick()?;
                    if let Some(cond) = cond {
                        if !self.eval(cond, fr)?.truthy() {
                            break;
                        }
                    }
                    match self.exec(body, fr)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                    if let Some(step) = step {
                        self.eval(step, fr)?;
                    }
                }
            }
            StmtKind::Foreach(index, var, array, body) => {
                let items = match self.eval(array, fr)? {
                    Value::Arr(items) => items,
                    Value::Vec(v) => Rc::new(v.iter().map(|c| Value::Float(*c)).collect()),
                    other => return Err(format!("foreach needs an array, got {}", other.type_name())),
                };
                for (i, item) in items.iter().enumerate() {
                    self.tick()?;
                    if let Some(index) = index {
                        fr.vals[*index] = Value::Int(i as i32);
                    }
                    store_local(fr, *var, item.clone())?;
                    match self.exec(body, fr)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            StmtKind::Block(stmts) => return self.exec_block(stmts, fr),
            StmtKind::Break => return Ok(Flow::Break),
            StmtKind::Continue => return Ok(Flow::Continue),
            StmtKind::Return(value) => {
                let value = match value {
                    Some(e) => self.eval(e, fr)?,
                    None => Value::Int(0),
                };
                return Ok(Flow::Return(value));
            }
        }
        Ok(Flow::Normal)
    }

    fn eval(&mut self, e: &'a Expr, fr: &mut Frame<'a>) -> R<Value> {
        Ok(match e {
            Expr::Const(v) => v.clone(),
            Expr::Local(slot) => fr.vals[*slot].clone(),
            Expr::Attr(id) => self.attr_read(*id)?,
            Expr::List(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval(item, fr)?);
                }
                Value::Arr(Rc::new(values))
            }
            Expr::Neg(a) => match self.eval(a, fr)? {
                Value::Int(i) => Value::Int(i.wrapping_neg()),
                Value::Float(f) => Value::Float(-f),
                Value::Str(_) => return Err("cannot negate a string".into()),
                other => Value::Vec(other.vec()?.map(|c| -c)),
            },
            Expr::Not(a) => Value::Int(!self.eval(a, fr)?.truthy() as i32),
            Expr::Bin(op, a, b) => {
                let a = self.eval(a, fr)?;
                let b = self.eval(b, fr)?;
                binop(*op, a, b)?
            }
            Expr::And(a, b) => {
                let result = self.eval(a, fr)?.truthy() && self.eval(b, fr)?.truthy();
                Value::Int(result as i32)
            }
            Expr::Or(a, b) => {
                let result = self.eval(a, fr)?.truthy() || self.eval(b, fr)?.truthy();
                Value::Int(result as i32)
            }
            Expr::Ternary(cond, a, b) => {
                if self.eval(cond, fr)?.truthy() {
                    self.eval(a, fr)?
                } else {
                    self.eval(b, fr)?
                }
            }
            Expr::Assign(place, op, rhs) => {
                let value = self.eval(rhs, fr)?;
                let value = match op {
                    Some(op) => {
                        let current = self.read_place(place, fr)?;
                        binop(*op, current, value)?
                    }
                    None => value,
                };
                self.write_place(place, value, fr)?
            }
            Expr::Step(place, delta, prefix) => {
                let current = self.read_place(place, fr)?;
                let stepped = binop(BinOp::Add, current.clone(), Value::Int(*delta))?;
                let stored = self.write_place(place, stepped, fr)?;
                if *prefix {
                    stored
                } else {
                    current
                }
            }
            Expr::Member(a, c) => component(&self.eval(a, fr)?, *c)?,
            Expr::Index(a, index) => {
                let base = self.eval(a, fr)?;
                let index = self.eval(index, fr)?.int()?;
                index_read(&base, index)?
            }
            Expr::Slice(a, start, end, step) => {
                let base = self.eval(a, fr)?;
                let mut part = |e: &'a Option<Box<Expr>>| -> R<Option<i32>> {
                    match e {
                        Some(e) => Ok(Some(self.eval(e, fr)?.int()?)),
                        None => Ok(None),
                    }
                };
                let (start, end, step) = (part(start)?, part(end)?, part(step)?);
                slice_value(&base, start, end, step.unwrap_or(1))?
            }
            Expr::Builtin(name, args, places, want_vec) => self.call_builtin(*name, args, places, *want_vec, fr)?,
            Expr::Call(index, args, places) => self.call_user(*index, args, places, fr)?,
            Expr::CallOverload(candidates, args, places) => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, fr)?);
                }
                let index = self.pick_overload(candidates, &values)?;
                self.call_user_values(index, values, places, fr)?
            }
        })
    }

    fn take_place(&mut self, place: &'a Place, fr: &mut Frame<'a>) -> R<Value> {
        match place {
            // Taking the value leaves the frame's copy unshared, so arrays mutate in place.
            Place::Local(slot) => Ok(std::mem::take(&mut fr.vals[*slot])),
            other => self.read_place(other, fr),
        }
    }

    fn read_place(&mut self, place: &'a Place, fr: &mut Frame<'a>) -> R<Value> {
        match place {
            Place::Local(slot) => Ok(fr.vals[*slot].clone()),
            Place::Attr(id) => self.attr_read(*id),
            Place::Member(base, c) => component(&self.read_place(base, fr)?, *c),
            Place::Index(base, index) => {
                let index = self.eval(index, fr)?.int()?;
                let base = self.read_place(base, fr)?;
                index_read(&base, index)
            }
        }
    }

    fn write_place(&mut self, place: &'a Place, value: Value, fr: &mut Frame<'a>) -> R<Value> {
        match place {
            Place::Local(slot) => store_local(fr, *slot, value),
            Place::Attr(id) => {
                self.attr_write(*id, &value)?;
                Ok(value)
            }
            Place::Member(base, c) => {
                let f = value.num()?;
                let current = self.take_place(base, fr)?;
                let updated = set_component(current, *c, f)?;
                self.write_place(base, updated, fr)?;
                Ok(Value::Float(f))
            }
            Place::Index(base, index) => {
                let index = self.eval(index, fr)?.int()?;
                let current = self.take_place(base, fr)?;
                let (updated, stored) = index_write(current, index, value)?;
                self.write_place(base, updated, fr)?;
                Ok(stored)
            }
        }
    }

    // The overload whose parameter types fit the arguments best: exact types beat conversions.
    fn pick_overload(&self, candidates: &[usize], values: &[Value]) -> R<usize> {
        let prog: &'a Program = self.prog;
        let score = |i: usize| -> Option<usize> {
            let f = &prog.functions[i];
            let mut total = 0;
            for (v, ty) in values.iter().zip(&f.param_types) {
                let exact = v.type_name() == ty.name() || (matches!(v, Value::Arr(_)) && ty.name().ends_with("[]"));
                // {a, b, c} with three numbers reads as a vector first, as in VEX.
                let triple = matches!(v, Value::Arr(items) if items.len() == 3 && items.iter().all(|i| matches!(i, Value::Int(_) | Value::Float(_))));
                if triple && *ty == Ty::Vector {
                    total += 3;
                } else if exact {
                    total += 2;
                } else if v.clone().convert(*ty).is_ok() {
                    total += 1;
                } else {
                    return None;
                }
            }
            Some(total)
        };
        let best = candidates.iter().filter_map(|&i| score(i).map(|s| (s, i))).max_by_key(|(s, i)| (*s, std::cmp::Reverse(*i)));
        best.map(|(_, i)| i).ok_or_else(|| {
            let got: Vec<&str> = values.iter().map(Value::type_name).collect();
            format!("no version of {}() takes ({})", prog.functions[candidates[0]].name, got.join(", "))
        })
    }

    fn call_user(&mut self, index: usize, args: &'a [Expr], places: &'a [Option<Place>], fr: &mut Frame<'a>) -> R<Value> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval(arg, fr)?);
        }
        self.call_user_values(index, values, places, fr)
    }

    fn call_user_values(&mut self, index: usize, args: Vec<Value>, places: &'a [Option<Place>], fr: &mut Frame<'a>) -> R<Value> {
        self.tick()?;
        if self.depth >= MAX_CALL_DEPTH {
            return Err(format!("too many nested function calls (more than {MAX_CALL_DEPTH}): runaway recursion?"));
        }
        let prog: &'a Program = self.prog;
        let f = &prog.functions[index];
        let mut vals: Vec<Value> = f.slot_types.iter().map(|t| t.map_or(Value::Int(0), Ty::default_value)).collect();
        for (i, value) in args.into_iter().enumerate() {
            vals[i] = value.convert(f.param_types[i]).map_err(|e| format!("argument {} of {}(): {e}", i + 1, f.name))?;
        }
        let mut frame = Frame { vals, types: &f.slot_types };
        self.depth += 1;
        let flow = self.exec_block(&f.body, &mut frame);
        self.depth -= 1;
        let result = match flow? {
            Flow::Return(v) => v,
            _ => Value::Int(0),
        };
        let result = match f.ret {
            Some(ty) => result.convert(ty).map_err(|e| format!("return value of {}(): {e}", f.name))?,
            None => Value::Int(0),
        };
        // Arguments are passed by reference: copy parameters back to assignable arguments.
        for (i, place) in places.iter().enumerate() {
            if let Some(place) = place {
                let value = std::mem::take(&mut frame.vals[i]);
                self.write_place(place, value, fr)?;
            }
        }
        Ok(result)
    }

    fn call_builtin(&mut self, name: &'static str, args: &'a [Expr], places: &'a [Option<Place>], want_vec: bool, fr: &mut Frame<'a>) -> R<Value> {
        if MUTATING.contains(&name) {
            let mut values = Vec::with_capacity(args.len());
            for arg in &args[1..] {
                values.push(self.eval(arg, fr)?);
            }
            let place = places[0].as_ref().expect("array place checked at compile time");
            let target = self.take_place(place, fr)?;
            let (updated, result) = mutate_array(name, target, &values).map_err(|e| format!("{name}(): {e}"))?;
            self.write_place(place, updated, fr)?;
            return Ok(result);
        }
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval(arg, fr)?);
        }
        if !out_params(name).is_empty() {
            let (result, outputs) = self.builtin_with_outputs(name, &values).map_err(|e| format!("{name}(): {e}"))?;
            for (i, value) in outputs {
                if let Some(Some(place)) = places.get(i) {
                    self.write_place(place, value, fr)?;
                }
            }
            return Ok(result);
        }
        self.builtin(name, &values, want_vec).map_err(|e| if name == "error" { e } else { format!("{name}(): {e}") })
    }

    fn attr_read(&mut self, id: usize) -> R<Value> {
        let prog: &'a Program = self.prog;
        let attr = &prog.attrs[id];
        let int = |n: usize| Value::Int(n as i32);
        Ok(match attr.kind {
            AttrKind::Ptnum => int(self.point_index().unwrap_or(0)),
            AttrKind::Primnum => int(match self.domain {
                Domain::Prim => self.prim,
                Domain::Vertex => self.poly_of.get(self.elem / 3).copied().unwrap_or(self.elem / 3),
                _ => 0,
            }),
            AttrKind::Vtxnum => int(if self.domain == Domain::Vertex { self.elem } else { 0 }),
            AttrKind::Numpt => int(self.input.vertices.len()),
            AttrKind::Numprim => int(self.polys.len()),
            AttrKind::Numvtx => int(self.input.indices.len() * 3),
            AttrKind::Elemnum => int(if self.domain == Domain::Prim { self.prim } else { self.elem }),
            AttrKind::Numelem => int(if self.domain == Domain::Prim { self.polys.len() } else { self.count }),
            AttrKind::Time => Value::Float(self.time),
            AttrKind::TimeInc => Value::Float(self.time_inc),
            // Frames count from 1 at time 0, as in Houdini.
            AttrKind::Frame => Value::Float(self.time * crate::sims::SIM_FPS + 1.0),
            AttrKind::P => match self.domain {
                Domain::Point | Domain::Vertex => Value::Vec(arr3(self.out.vertices[self.point_index().unwrap_or(0)])),
                Domain::Prim => Value::Vec(arr3(crate::poly::centroid(&self.out, &self.polys[self.prim]))),
                Domain::Detail => read_storage(&self.out, &self.keys[id], 0, 1, Some(Ty::Vector)),
            },
            AttrKind::N => match self.domain {
                Domain::Point | Domain::Vertex => point_normal(&self.out, self.normals, self.point_index().unwrap_or(0)),
                Domain::Prim => Value::Vec(arr3(crate::poly::normal(&self.out, &self.polys[self.prim]))),
                Domain::Detail => read_storage(&self.out, &self.keys[id], 0, 1, Some(Ty::Vector)),
            },
            _ => read_storage(&self.out, &self.keys[id], self.elem, self.count, attr.hint),
        })
    }

    fn attr_write(&mut self, id: usize, value: &Value) -> R<()> {
        let prog: &'a Program = self.prog;
        let attr = &prog.attrs[id];
        match (attr.kind, self.domain) {
            (AttrKind::P, Domain::Point | Domain::Vertex) => {
                let pt = self.point_index().unwrap_or(0);
                self.out.vertices[pt] = vec3(value.vec()?);
            }
            (AttrKind::P, Domain::Prim) => {
                // Moving a polygon's centre moves its points.
                let run = self.polys[self.prim].clone();
                let delta = vec3(value.vec()?).sub(crate::poly::centroid(&self.out, &run));
                for pt in crate::poly::points(&self.out, &run) {
                    self.out.vertices[pt] = self.out.vertices[pt].add(delta);
                }
            }
            (AttrKind::N, Domain::Point | Domain::Vertex) => {
                let pt = self.point_index().unwrap_or(0);
                let count = self.out.vertices.len();
                write_storage(&mut self.out, "N", pt, count, value, Some(Ty::Vector), "N")?;
            }
            _ => write_storage(&mut self.out, &self.keys[id], self.elem, self.count, value, attr.hint, &attr.name)?,
        }
        Ok(())
    }

    // Reads an attribute of the input geometry (point(), prim(), vertex(), detail()).
    fn input_attr(&self, domain: Domain, name: &str, index: i32) -> Value {
        let mesh = self.input;
        let hint = known_hint(name);
        // Primitive numbers are polygons: read the polygon's first triangle.
        if domain == Domain::Prim {
            let Some(run) = usize::try_from(index).ok().and_then(|p| self.polys.get(p)) else {
                return hint.filter(|t| *t != Ty::Str).map_or(Value::Float(0.0), Ty::default_value);
            };
            return match name {
                "primnum" => Value::Int(index),
                "P" => Value::Vec(arr3(crate::poly::centroid(mesh, run))),
                "N" => Value::Vec(arr3(crate::poly::normal(mesh, run))),
                _ => read_storage(mesh, &format!("prim:{name}"), run.start, mesh.indices.len(), hint),
            };
        }
        let count = domain.count(mesh);
        let Some(index) = usize::try_from(index).ok().filter(|i| *i < count) else {
            return hint.filter(|t| *t != Ty::Str).map_or(Value::Float(0.0), Ty::default_value);
        };
        match (domain, name) {
            (Domain::Point, "P") => Value::Vec(arr3(mesh.vertices[index])),
            (Domain::Point, "N") => point_normal(mesh, self.normals, index),
            (Domain::Point, "ptnum") | (Domain::Prim, "primnum") | (Domain::Vertex, "vtxnum") => Value::Int(index as i32),
            (Domain::Prim, "P") => Value::Vec(arr3(tri_centroid(mesh, mesh.indices[index]))),
            (Domain::Prim, "N") => Value::Vec(arr3(tri_normal(mesh, mesh.indices[index]))),
            (Domain::Vertex, "P") => Value::Vec(arr3(mesh.vertices[mesh.indices[index / 3][index % 3]])),
            _ => read_storage(mesh, &format!("{}{name}", domain.prefix()), index, count, hint),
        }
    }

    fn channel(&self, name: &str) -> [f32; 3] {
        self.channels.and_then(|c| c.get(name)).copied().unwrap_or([0.0; 3])
    }

    fn grid(&mut self) -> &PointGrid {
        let input = self.input;
        self.point_grid.get_or_insert_with(|| PointGrid::new(&input.vertices))
    }

    fn pc_handle(&self, handle: &Value) -> R<&PcHandle> {
        let index = handle.int()?;
        usize::try_from(index).ok().and_then(|i| self.pc_handles.get(i)).ok_or_else(|| format!("{index} is not an open point cloud handle"))
    }

    fn pc_handle_mut(&mut self, handle: &Value) -> R<&mut PcHandle> {
        let index = handle.int()?;
        usize::try_from(index).ok().and_then(|i| self.pc_handles.get_mut(i)).ok_or_else(|| format!("{index} is not an open point cloud handle"))
    }

    // Builtins with output arguments: the result plus (argument index, value) to write back.
    fn builtin_with_outputs(&mut self, name: &str, a: &[Value]) -> R<(Value, Vec<(usize, Value)>)> {
        Ok(match name {
            "xyzdist" => {
                let p = vec3(a[1].vec()?);
                match closest_on_surface(self.input, p) {
                    Some(hit) if hit.prim == usize::MAX => (Value::Float(hit.position.sub(p).length()), vec![(2, Value::Int(-1)), (3, Value::Vec([hit.uv[0], hit.uv[1], 0.0]))]),
                    Some(hit) => {
                        let (prim, uv) = self.triangle_to_poly_uv(hit.prim, hit.uv);
                        (Value::Float(hit.position.sub(p).length()), vec![(2, Value::Int(prim as i32)), (3, Value::Vec(uv))])
                    }
                    None => (Value::Float(-1.0), vec![(2, Value::Int(-1))]),
                }
            }
            "intersect" => {
                let origin = vec3(a[1].vec()?);
                let dir = vec3(a[2].vec()?);
                match ray_hit(self.input, origin, dir) {
                    Some(hit) => {
                        let (prim, uv) = self.triangle_to_poly_uv(hit.prim, hit.uv);
                        (Value::Int(prim as i32), vec![(3, Value::Vec(arr3(hit.position))), (4, Value::Vec(uv))])
                    }
                    None => (Value::Int(-1), vec![(3, Value::Vec(arr3(origin.add(dir))))]),
                }
            }
            "pcimport" => {
                let attr = a[1].string()?;
                let current = {
                    let handle = self.pc_handle(&a[0])?;
                    handle.current.map(|i| handle.found[i])
                };
                match current {
                    Some((pt, distance)) => {
                        let value = match &*attr {
                            "point.number" => Value::Int(pt as i32),
                            "point.distance" => Value::Float(distance),
                            _ => self.input_attr(Domain::Point, &attr, pt as i32),
                        };
                        (Value::Int(1), vec![(2, value)])
                    }
                    None => (Value::Int(0), Vec::new()),
                }
            }
            "translate" | "rotate" | "scale" => {
                use crate::vexmath as vm;
                let matrix = |m: &Value, step: &[f32]| -> R<Value> {
                    Ok(match m {
                        Value::Mat3(x) if step.len() == 9 => Value::Mat3(vm::m3_mul(x, &step.try_into().unwrap())),
                        Value::Mat3(_) => return Err("a matrix3 can't hold a translation (use a matrix)".into()),
                        other => Value::Mat4(vm::m4_mul(&other.mat4()?, &if step.len() == 9 { vm::m3_to_m4(&step.try_into().unwrap()) } else { step.try_into().unwrap() })),
                    })
                };
                let step: Vec<f32> = match name {
                    "translate" => vm::translate_m4(a[1].vec()?).to_vec(),
                    "rotate" => vm::rotation_m3(a[1].num()?, a[2].vec()?).to_vec(),
                    _ => vm::scale_m3(a[1].vec()?).to_vec(),
                };
                if !matches!(a[0], Value::Mat3(_) | Value::Mat4(_)) {
                    return Err(format!("the first argument must be a matrix3 or matrix variable, got {}", a[0].type_name()));
                }
                let updated = matrix(&a[0], &step)?;
                (Value::Int(0), vec![(0, updated)])
            }
            "setcomp" => {
                let value = a[1].num()?;
                let updated = match (&a[0], a.len()) {
                    (Value::Mat3(m), 4) => {
                        let mut m = *m;
                        m[(a[2].int()?.clamp(0, 2) * 3 + a[3].int()?.clamp(0, 2)) as usize] = value;
                        Value::Mat3(m)
                    }
                    (Value::Mat4(m), 4) => {
                        let mut m = *m;
                        m[(a[2].int()?.clamp(0, 3) * 4 + a[3].int()?.clamp(0, 3)) as usize] = value;
                        Value::Mat4(m)
                    }
                    (v, 3) => index_write(v.clone(), a[2].int()?, Value::Float(value))?.0,
                    (v, _) => return Err(format!("setcomp on a {} takes a row and a column", v.type_name())),
                };
                (Value::Int(0), vec![(0, updated)])
            }
            other => return Err(format!("{other} has no outputs")),
        })
    }

    fn neighbour_lists(&mut self) -> &Vec<Vec<usize>> {
        let input = self.input;
        self.neighbours.get_or_insert_with(|| {
            let mut lists = vec![Vec::new(); input.vertices.len()];
            for tri in &input.indices {
                for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    if a < lists.len() && b < lists.len() {
                        lists[a].push(b);
                        lists[b].push(a);
                    }
                }
            }
            for list in &mut lists {
                list.sort_unstable();
                list.dedup();
            }
            lists
        })
    }

    // A point's number, or every triangle of a polygon (what setprimattrib & co. write to).
    fn elements_of(&self, domain: Domain, index: usize) -> Vec<usize> {
        match domain {
            Domain::Prim => self.polys.get(index).map_or_else(Vec::new, |run| run.clone().collect()),
            _ => vec![index],
        }
    }

    // A hit on a triangle as its polygon and a polygon uv (see poly_uv_to_triangle).
    fn triangle_to_poly_uv(&self, tri: usize, uv: [f32; 2]) -> (usize, [f32; 3]) {
        let p = self.poly_of.get(tri).copied().unwrap_or(tri);
        let run = self.polys.get(p).cloned().unwrap_or(tri..tri + 1);
        let n = run.len() as f32;
        (p, [((tri - run.start) as f32 + uv[0]) / n, uv[1], 0.0])
    }

    fn point_prim_lists(&mut self) -> &Vec<Vec<usize>> {
        let input = self.input;
        self.point_prims.get_or_insert_with(|| {
            let mut lists = vec![Vec::new(); input.vertices.len()];
            for (t, tri) in input.indices.iter().enumerate() {
                for &p in tri {
                    if p < lists.len() && lists[p].last() != Some(&t) {
                        lists[p].push(t);
                    }
                }
            }
            lists
        })
    }

    fn bounds(&self) -> (Vec3, Vec3) {
        self.input.bounds().unwrap_or((Vec3::zero(), Vec3::zero()))
    }

    fn builtin(&mut self, name: &str, a: &[Value], want_vec: bool) -> R<Value> {
        use Value::{Float, Int, Str, Vec as V};
        let index_of = |v: &Value| -> R<usize> { usize::try_from(v.int()?).map_err(|_| "index must not be negative".to_string()) };
        Ok(match name {
            "abs" if all_int(a) => Int(a[0].int()?.wrapping_abs()),
            "abs" => per_lane(a, |x| x[0].abs())?,
            "sign" => per_lane(a, |x| if x[0] > 0.0 { 1.0 } else if x[0] < 0.0 { -1.0 } else { 0.0 })?,
            "floor" => per_lane(a, |x| x[0].floor())?,
            "ceil" => per_lane(a, |x| x[0].ceil())?,
            "round" => per_lane(a, |x| x[0].round())?,
            "trunc" => per_lane(a, |x| x[0].trunc())?,
            "frac" => per_lane(a, |x| x[0] - x[0].floor())?,
            "sqrt" => per_lane(a, |x| x[0].max(0.0).sqrt())?,
            "exp" => per_lane(a, |x| x[0].exp())?,
            "log" => per_lane(a, |x| x[0].max(1e-30).ln())?,
            "log10" => per_lane(a, |x| x[0].max(1e-30).log10())?,
            "pow" => per_lane(a, |x| x[0].powf(x[1]))?,
            "sin" => per_lane(a, |x| x[0].sin())?,
            "cos" => per_lane(a, |x| x[0].cos())?,
            "tan" => per_lane(a, |x| x[0].tan())?,
            "asin" => per_lane(a, |x| x[0].clamp(-1.0, 1.0).asin())?,
            "acos" => per_lane(a, |x| x[0].clamp(-1.0, 1.0).acos())?,
            "atan" if a.len() == 2 => per_lane(a, |x| x[0].atan2(x[1]))?,
            "atan" => per_lane(a, |x| x[0].atan())?,
            "atan2" => per_lane(a, |x| x[0].atan2(x[1]))?,
            "sinh" => per_lane(a, |x| x[0].sinh())?,
            "cosh" => per_lane(a, |x| x[0].cosh())?,
            "tanh" => per_lane(a, |x| x[0].tanh())?,
            "radians" => per_lane(a, |x| x[0].to_radians())?,
            "degrees" => per_lane(a, |x| x[0].to_degrees())?,
            "min" | "max" => {
                let items = if a.len() == 1 { a[0].items()? } else { Rc::new(a.to_vec()) };
                let mut best = items.first().cloned().ok_or("needs at least one value")?;
                for item in items.iter().skip(1) {
                    best = pick_extreme(best, item.clone(), name == "min")?;
                }
                best
            }
            "clamp" if all_int(a) => Int(a[0].int()?.max(a[1].int()?).min(a[2].int()?)),
            "clamp" => per_lane(a, |x| x[0].max(x[1]).min(x[2]))?,
            "mod" if all_int(a) => binop(BinOp::Mod, a[0].clone(), a[1].clone())?,
            "mod" => per_lane(a, |x| if x[1] == 0.0 { 0.0 } else { x[0] % x[1] })?,
            "fit" | "fitclamp" => per_lane(a, |x| fit(x[0], x[1], x[2], x[3], x[4], true))?,
            "efit" => per_lane(a, |x| fit(x[0], x[1], x[2], x[3], x[4], false))?,
            "fit01" => per_lane(a, |x| fit(x[0], 0.0, 1.0, x[1], x[2], true))?,
            "fit10" => per_lane(a, |x| fit(x[0], 1.0, 0.0, x[1], x[2], true))?,
            "fit11" => per_lane(a, |x| fit(x[0], -1.0, 1.0, x[1], x[2], true))?,
            "lerp" => per_lane(a, |x| x[0] + (x[1] - x[0]) * x[2])?,
            "smooth" | "smoothstep" => per_lane(a, |x| smoothstep(x[0], x[1], x[2]))?,

            "set" | "vector" if a.len() == 3 => V([a[0].num()?, a[1].num()?, a[2].num()?]),
            "set" | "vector" if a.len() == 1 => V(a[0].vec()?),
            "set" | "vector4" if a.len() == 4 => Value::Vec4([a[0].num()?, a[1].num()?, a[2].num()?, a[3].num()?]),
            "set" | "matrix3" if a.len() == 9 => Value::Mat3(std::array::from_fn(|i| a[i].num().unwrap_or(0.0))),
            "set" | "matrix" if a.len() == 16 => Value::Mat4(std::array::from_fn(|i| a[i].num().unwrap_or(0.0))),
            "vector4" if a.len() == 1 => Value::Vec4(a[0].vec4()?),
            "matrix3" if a.len() == 1 => Value::Mat3(a[0].mat3()?),
            "matrix" if a.len() == 1 => Value::Mat4(a[0].mat4()?),
            "set" | "vector" => return Err("expects 1, 3, 4, 9 or 16 arguments".into()),
            "vector4" | "matrix3" | "matrix" => return Err(format!("{name}() takes one value or all its components")),
            "length" | "normalize" | "normalizesafe" | "length2" | "dot" if matches!(a[0], Value::Vec4(_)) => {
                let q = a[0].vec4()?;
                let len2: f32 = q.iter().map(|c| c * c).sum();
                match name {
                    "length" => Float(len2.sqrt()),
                    "length2" => Float(len2),
                    "dot" => Float(q.iter().zip(a[1].vec4()?).map(|(x, y)| x * y).sum()),
                    _ => Value::Vec4(crate::vexmath::q_normalize(q)),
                }
            }
            "length" => Float(vec3(a[0].vec()?).length()),
            "length2" => {
                let v = vec3(a[0].vec()?);
                Float(v.dot(v))
            }
            "normalize" | "normalizesafe" => V(arr3(vec3(a[0].vec()?).normalize())),
            "dot" => Float(vec3(a[0].vec()?).dot(vec3(a[1].vec()?))),
            "cross" => V(arr3(vec3(a[0].vec()?).cross(vec3(a[1].vec()?)))),
            "distance" => Float(vec3(a[0].vec()?).sub(vec3(a[1].vec()?)).length()),
            "distance2" => {
                let d = vec3(a[0].vec()?).sub(vec3(a[1].vec()?));
                Float(d.dot(d))
            }
            "reflect" => {
                let (i, n) = (vec3(a[0].vec()?), vec3(a[1].vec()?).normalize());
                V(arr3(i.sub(n.scale(2.0 * i.dot(n)))))
            }
            "refract" => {
                let (i, n, eta) = (vec3(a[0].vec()?).normalize(), vec3(a[1].vec()?).normalize(), a[2].num()?);
                let cos = n.dot(i);
                let k = 1.0 - eta * eta * (1.0 - cos * cos);
                V(if k < 0.0 { [0.0; 3] } else { arr3(i.scale(eta).sub(n.scale(eta * cos + k.sqrt()))) })
            }
            "avg" if a.len() == 1 => match &a[0] {
                Value::Arr(items) => average(items)?,
                V(v) => Float((v[0] + v[1] + v[2]) / 3.0),
                other => Float(other.num()?),
            },
            "avg" => average(a)?,
            "luminance" => {
                let c = a[0].vec()?;
                Float(0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
            }
            "hsvtorgb" => V(hsv_to_rgb(a[0].vec()?)),
            "rgbtohsv" => V(rgb_to_hsv(a[0].vec()?)),

            "rand" | "random" => {
                let mut seed = seed_bits(&a[0])?;
                if let Some(second) = a.get(1) {
                    seed ^= seed_bits(second)?.wrapping_mul(0x9E37_79B9);
                }
                if want_vec {
                    V([hash01(seed, 17), hash01(seed, 29), hash01(seed, 43)])
                } else {
                    Float(hash01(seed, 17))
                }
            }
            "noise" | "snoise" => {
                let p = match a {
                    [single] if single.is_vector_like() => single.vec()?,
                    [x] => [x.num()?, 0.0, 0.0],
                    [x, y] => [x.num()?, y.num()?, 0.0],
                    _ => [a[0].num()?, a[1].num()?, a[2].num()?],
                };
                let shape = |n: f32| if name == "noise" { (n * 0.5 + 0.5).clamp(0.0, 1.0) } else { n.clamp(-1.0, 1.0) };
                if want_vec {
                    V([shape(perlin3(p, 0)), shape(perlin3(p, 1)), shape(perlin3(p, 2))])
                } else {
                    Float(shape(perlin3(p, 0)))
                }
            }

            "int" => match &a[0] {
                Str(s) => Int(s.trim().parse::<f64>().map(|f| f as i32).unwrap_or(0)),
                other => Int(other.int()?),
            },
            "float" => match &a[0] {
                Str(s) => Float(s.trim().parse().unwrap_or(0.0)),
                other => Float(other.num()?),
            },
            "string" => Str(a[0].display().into()),
            "itoa" => Str(a[0].int()?.to_string().into()),
            "atoi" => Int(a[0].string()?.trim().parse::<f64>().map(|f| f as i32).unwrap_or(0)),
            "atof" => Float(a[0].string()?.trim().parse().unwrap_or(0.0)),

            "sprintf" => Str(format_vex(a)?.into()),
            "printf" => {
                let text = format_vex(a)?;
                self.print(&text);
                Int(0)
            }
            "warning" => {
                let text = format_vex(a)?;
                self.push_log(format!("Warning: {}", text.trim_end()));
                Int(0)
            }
            "error" => return Err(format_vex(a)?.trim_end().to_string()),
            "concat" => Str(a.iter().map(Value::display).collect::<String>().into()),
            "strlen" => Int(a[0].string()?.chars().count() as i32),
            "toupper" => Str(a[0].string()?.to_uppercase().into()),
            "tolower" => Str(a[0].string()?.to_lowercase().into()),
            "startswith" => Int(a[0].string()?.starts_with(&*a[1].string()?) as i32),
            "endswith" => Int(a[0].string()?.ends_with(&*a[1].string()?) as i32),
            "replace" => Str(a[0].string()?.replace(&*a[1].string()?, &a[2].string()?).into()),
            "split" => {
                let s = a[0].string()?;
                let parts: Vec<Value> = match a.get(1) {
                    Some(sep) => {
                        let sep = sep.string()?;
                        s.split(|c| sep.contains(c)).filter(|p| !p.is_empty()).map(|p| Str(p.into())).collect()
                    }
                    None => s.split_whitespace().map(|p| Str(p.into())).collect(),
                };
                Value::Arr(Rc::new(parts))
            }
            "join" => Str(a[0].items()?.iter().map(Value::display).collect::<Vec<_>>().join(&a[1].string()?).into()),

            "array" => Value::Arr(Rc::new(a.to_vec())),
            "len" => Int((match &a[0] {
                Value::Arr(items) => items.len(),
                Str(s) => s.chars().count(),
                V(_) => 3,
                _ => 1,
            }) as i32),
            "sort" => {
                let items = a[0].items()?;
                let order = sort_values(&items)?;
                Value::Arr(Rc::new(order.into_iter().map(|i| items[i].clone()).collect()))
            }
            "argsort" => int_array(sort_values(&a[0].items()?)?),
            "reverse" => match &a[0] {
                Str(s) => Str(s.chars().rev().collect::<String>().into()),
                other => Value::Arr(Rc::new(other.items()?.iter().rev().cloned().collect())),
            },
            "sum" => match &a[0] {
                V(v) => Float(v[0] + v[1] + v[2]),
                other => {
                    let items = other.items()?;
                    let mut total = Int(0);
                    for item in items.iter() {
                        total = binop(BinOp::Add, total, item.clone())?;
                    }
                    total
                }
            },
            "find" => match &a[0] {
                Str(s) => {
                    let needle = a[1].string()?;
                    Int(s.find(&*needle).map_or(-1, |byte| s[..byte].chars().count() as i32))
                }
                other => Int(other.items()?.iter().position(|v| values_equal(v, &a[1])).map_or(-1, |i| i as i32)),
            },
            "isvalidindex" => {
                let len = a[0].items()?.len() as i64;
                let i = a[1].int()? as i64;
                Int((i >= 0 && i < len) as i32)
            }

            "npoints" => Int(self.input.vertices.len() as i32),
            "nprimitives" => Int(self.polys.len() as i32),
            "nvertices" => Int(self.input.indices.len() as i32 * 3),
            "point" => self.input_attr(Domain::Point, &a[1].string()?, a[2].int()?),
            "prim" => self.input_attr(Domain::Prim, &a[1].string()?, a[2].int()?),
            "vertex" => self.input_attr(Domain::Vertex, &a[1].string()?, a[2].int()?),
            "detail" => self.input_attr(Domain::Detail, &a[1].string()?, 0),
            "haspointattrib" => {
                let attr = a[1].string()?;
                Int((matches!(&*attr, "P" | "N") || self.input.point_attributes.contains_key(&*attr)) as i32)
            }
            "inpointgroup" | "inprimgroup" => {
                let (domain, key) = if name == "inpointgroup" {
                    (Domain::Point, format!("group:{}", a[1].string()?))
                } else {
                    (Domain::Prim, format!("group:{}", a[1].string()?))
                };
                Int((self.input_attr(domain, &key, a[2].int()?).num().unwrap_or(0.0) > 0.5) as i32)
            }
            "setpointgroup" | "setprimgroup" => {
                let domain = if name == "setpointgroup" { Domain::Point } else { Domain::Prim };
                let index = index_of(&a[2])?;
                let on = Value::Float(if a[3].truthy() { 1.0 } else { 0.0 });
                let key = format!("group:{}", a[1].string()?);
                for element in self.elements_of(domain, index) {
                    self.pending.writes.push((domain, key.clone(), element, on.clone(), write_mode(None)?));
                }
                Int(index as i32)
            }
            "hasprimattrib" => Int(self.input.point_attributes.contains_key(&format!("prim:{}", a[1].string()?)) as i32),
            "setpointattrib" | "setprimattrib" => {
                let domain = if name == "setpointattrib" { Domain::Point } else { Domain::Prim };
                let index = index_of(&a[2])?;
                let mode = write_mode(a.get(4))?;
                let attr = a[1].string()?.to_string();
                for element in self.elements_of(domain, index) {
                    self.pending.writes.push((domain, attr.clone(), element, a[3].clone(), mode));
                }
                Int(index as i32)
            }
            "setdetailattrib" => {
                let mode = write_mode(a.get(3))?;
                self.pending.writes.push((Domain::Detail, a[1].string()?.to_string(), 0, a[2].clone(), mode));
                Int(0)
            }
            "addpoint" => {
                let position = match &a[1] {
                    Int(pt) => self.input.vertices.get(*pt as usize).copied().map(arr3).unwrap_or([0.0; 3]),
                    other => other.vec()?,
                };
                let index = self.input.vertices.len() + self.pending.add_points.len();
                self.pending.add_points.push(position);
                Int(index as i32)
            }
            "addprim" => {
                let kind = a[1].string()?;
                if &*kind != "poly" {
                    return Err(format!("only \"poly\" primitives are supported, got \"{kind}\""));
                }
                // The points listed, or one array of them.
                let pts = match &a[2] {
                    Value::Arr(list) if a.len() == 3 => list.iter().map(index_of).collect::<R<Vec<usize>>>()?,
                    _ => a[2..].iter().map(index_of).collect::<R<Vec<usize>>>()?,
                };
                // One polygon, of any number of points.
                let index = self.polys.len() + self.pending.add_prims.len();
                self.pending.add_prims.push(pts);
                Int(index as i32)
            }
            "removepoint" => {
                if let Ok(pt) = usize::try_from(a[1].int()?) {
                    self.pending.remove_points.insert(pt);
                }
                Int(1)
            }
            "removeprim" => {
                if let Some(run) = usize::try_from(a[1].int()?).ok().and_then(|p| self.polys.get(p)).cloned() {
                    self.pending.remove_prims.extend(run.clone());
                    if a.get(2).is_some_and(Value::truthy) {
                        self.pending.orphan_candidates.extend(crate::poly::points(self.input, &run));
                    }
                }
                Int(1)
            }
            // ---- matrices and quaternions ----
            "ident" => Value::Mat4(crate::vexmath::m4_ident()),
            "transpose" => match &a[0] {
                Value::Mat3(m) => Value::Mat3(crate::vexmath::transpose::<3>(m).try_into().unwrap()),
                other => Value::Mat4(crate::vexmath::transpose::<4>(&other.mat4()?).try_into().unwrap()),
            },
            "invert" => match &a[0] {
                Value::Mat3(m) => Value::Mat3(crate::vexmath::invert(m, 3).map_or([0.0; 9], |v| v.try_into().unwrap())),
                Value::Mat4(m) => Value::Mat4(crate::vexmath::invert(m, 4).map_or([0.0; 16], |v| v.try_into().unwrap())),
                other => Float(match other.num()? {
                    0.0 => 0.0,
                    x => 1.0 / x,
                }),
            },
            "determinant" => match &a[0] {
                Value::Mat3(m) => Float(crate::vexmath::det(m, 3)),
                other => Float(crate::vexmath::det(&other.mat4()?, 4)),
            },
            "getcomp" => match (&a[0], a.len()) {
                (Value::Mat3(m), 3) => Float(m[(a[1].int()?.clamp(0, 2) * 3 + a[2].int()?.clamp(0, 2)) as usize]),
                (Value::Mat4(m), 3) => Float(m[(a[1].int()?.clamp(0, 3) * 4 + a[2].int()?.clamp(0, 3)) as usize]),
                (v, _) => index_read(v, a[1].int()?)?,
            },
            "maketransform" => {
                let (trs, xyz, rest) = if a.len() == 5 { (a[0].int()?, a[1].int()?, &a[2..]) } else { (0, 0, a) };
                if rest.len() != 3 {
                    return Err("expects (t, r, s) or (trs, xyz, t, r, s)".into());
                }
                Value::Mat4(crate::vexmath::make_transform(trs, xyz, rest[0].vec()?, rest[1].vec()?, rest[2].vec()?))
            }
            "lookat" => {
                let up = a.get(2).map(Value::vec).transpose()?.unwrap_or([0.0, 1.0, 0.0]);
                Value::Mat3(crate::vexmath::lookat(a[0].vec()?, a[1].vec()?, up))
            }
            "quaternion" => match (a.len(), &a[0]) {
                (2, _) => Value::Vec4(crate::vexmath::q_axis_angle(a[0].num()?, a[1].vec()?)),
                (_, m @ (Value::Mat3(_) | Value::Mat4(_))) => Value::Vec4(crate::vexmath::q_from_m3(&m.mat3()?)),
                (_, v) => {
                    // angle * axis
                    let v = v.vec()?;
                    let angle = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    Value::Vec4(if angle > 0.0 { crate::vexmath::q_axis_angle(angle, v) } else { [0.0, 0.0, 0.0, 1.0] })
                }
            },
            "qrotate" => V(crate::vexmath::q_rotate(&a[0].vec4()?, a[1].vec()?)),
            "qmultiply" => Value::Vec4(crate::vexmath::q_mul(&a[0].vec4()?, &a[1].vec4()?)),
            "qinvert" => Value::Vec4(crate::vexmath::q_invert(&a[0].vec4()?)),
            "qconvert" => Value::Mat3(crate::vexmath::q_to_m3(&a[0].vec4()?)),
            "slerp" => Value::Vec4(crate::vexmath::slerp(&a[0].vec4()?, &a[1].vec4()?, a[2].num()?)),
            "dihedral" => Value::Vec4(crate::vexmath::dihedral(a[0].vec()?, a[1].vec()?)),
            "eulertoquaternion" => Value::Vec4(crate::vexmath::euler_to_q(a[0].vec()?, a.get(1).map(Value::int).transpose()?.unwrap_or(0))),
            "quaterniontoeuler" => V(crate::vexmath::q_to_euler(&a[0].vec4()?, a.get(1).map(Value::int).transpose()?.unwrap_or(0))),
            // ---- noise, maths ----
            "wnoise" => {
                let p = a[0].vec()?;
                let seed = a.get(1).map(Value::int).transpose()?.unwrap_or(0) as u32;
                Float(worley(p, seed))
            }
            "anoise" => {
                let p = a[0].vec()?;
                let ridged = |s: u32| -> f32 {
                    let (mut total, mut amp, mut freq, mut norm) = (0.0, 0.5, 1.0, 0.0);
                    for octave in 0..4 {
                        let n = perlin3([p[0] * freq, p[1] * freq, p[2] * freq], s * 7 + octave);
                        total += amp * (1.0 - n.abs()).powi(2);
                        norm += amp;
                        amp *= 0.5;
                        freq *= 2.03;
                    }
                    (total / norm).clamp(0.0, 1.0)
                };
                if want_vec { V([ridged(0), ridged(1), ridged(2)]) } else { Float(ridged(0)) }
            }
            "cbrt" => per_lane(a, |x| x[0].cbrt())?,
            "invlerp" => per_lane(a, |x| if (x[1] - x[0]).abs() < 1e-20 { 0.0 } else { (x[2] - x[0]) / (x[1] - x[0]) })?,
            "isnan" => Int(a[0].num()?.is_nan() as i32),
            "isfinite" => Int(a[0].num()?.is_finite() as i32),
            "spline" => {
                let basis = a[0].string()?;
                let t = a[1].num()?.clamp(0.0, 1.0);
                let keys = &a[2..];
                if keys.is_empty() {
                    return Err("needs at least one value".into());
                }
                let vector = keys.iter().any(Value::is_vector_like);
                let get = |i: usize| -> R<[f32; 3]> { if vector { keys[i].vec() } else { keys[i].num().map(|f| [f; 3]) } };
                let lerp = |x: [f32; 3], y: [f32; 3], u: f32| [0, 1, 2].map(|c| x[c] + (y[c] - x[c]) * u);
                let out = match &*basis {
                    "constant" => get(((t * keys.len() as f32) as usize).min(keys.len() - 1))?,
                    "catrom" if keys.len() >= 4 => {
                        // v0 and the last value only shape the ends; the curve passes v1..v(n-2).
                        let segments = keys.len() - 3;
                        let f = t * segments as f32;
                        let s = (f as usize).min(segments - 1);
                        let u = f - s as f32;
                        let (p0, p1, p2, p3) = (get(s)?, get(s + 1)?, get(s + 2)?, get(s + 3)?);
                        [0, 1, 2].map(|c| {
                            0.5 * (2.0 * p1[c] + (p2[c] - p0[c]) * u + (2.0 * p0[c] - 5.0 * p1[c] + 4.0 * p2[c] - p3[c]) * u * u
                                + (3.0 * p1[c] - p0[c] - 3.0 * p2[c] + p3[c]) * u * u * u)
                        })
                    }
                    "linear" | "catrom" => {
                        if keys.len() == 1 {
                            get(0)?
                        } else {
                            let f = t * (keys.len() - 1) as f32;
                            let s = (f as usize).min(keys.len() - 2);
                            lerp(get(s)?, get(s + 1)?, f - s as f32)
                        }
                    }
                    other => return Err(format!("unknown basis \"{other}\" (use \"linear\", \"catrom\" or \"constant\")")),
                };
                if vector { V(out) } else { Float(out[0]) }
            }
            "ch" | "chf" => Float(self.channel(&a[0].string()?)[0]),
            "chi" => Int(self.channel(&a[0].string()?)[0].round() as i32),
            "chv" => V(self.channel(&a[0].string()?)),
            // ---- strings ----
            "strip" | "lstrip" | "rstrip" => {
                let text = a[0].string()?;
                let chars: Vec<char> = match a.get(1) {
                    Some(c) => c.string()?.chars().collect(),
                    None => vec![' ', '\t', '\n', '\r'],
                };
                let strip = |c: char| chars.contains(&c);
                Str(match name {
                    "lstrip" => text.trim_start_matches(strip),
                    "rstrip" => text.trim_end_matches(strip),
                    _ => text.trim_matches(strip),
                }
                .into())
            }
            "match" => Int(glob(&a[0].string()?, &a[1].string()?) as i32),
            "re_match" | "re_find" | "re_replace" | "re_split" => {
                let re = cached_regex(&a[0].string()?)?;
                match name {
                    "re_match" => Int(re.is_match(&a[1].string()?) as i32),
                    "re_find" => Str(re.find(&a[1].string()?).map_or("", |m| m.as_str()).into()),
                    "re_replace" => Str(re.replace_all(&a[2].string()?, &*a[1].string()?).into_owned().into()),
                    _ => Value::Arr(Rc::new(re.split(&a[1].string()?).map(|p| Str(p.into())).collect())),
                }
            }
            "padzero" => {
                let width = a[0].int()?.max(0) as usize;
                let n = a[1].int()?;
                Str(if n < 0 { format!("-{:0width$}", -n, width = width.saturating_sub(1)) } else { format!("{n:0width$}") }.into())
            }
            // ---- geometry ----
            "primuv" => {
                let attr = a[1].string()?;
                let prim = a[2].int()?;
                let Some((t, uv)) = usize::try_from(prim).ok().and_then(|p| self.polys.get(p)).map(|run| poly_uv_to_triangle(run, a[3].vec().unwrap_or([0.0; 3]))) else {
                    return Ok(Float(0.0));
                };
                let tri = self.input.indices[t];
                let weights = [1.0 - uv[0] - uv[1], uv[0], uv[1]];
                let prim_key = format!("prim:{attr}");
                if &*attr != "P" && self.input.point_attributes.contains_key(&prim_key) {
                    self.input_attr(Domain::Prim, &attr, prim)
                } else {
                    let corners: Vec<Value> = tri.iter().map(|&p| self.input_attr(Domain::Point, &attr, p as i32)).collect();
                    match &corners[0] {
                        Value::Vec4(_) => Value::Vec4(std::array::from_fn(|c| corners.iter().zip(weights).map(|(v, w)| v.vec4().map_or(0.0, |q| q[c]) * w).sum())),
                        v if v.is_vector_like() => V(std::array::from_fn(|c| corners.iter().zip(weights).map(|(v, w)| v.vec().map_or(0.0, |x| x[c]) * w).sum())),
                        _ => Float(corners.iter().zip(weights).map(|(v, w)| v.num().unwrap_or(0.0) * w).sum()),
                    }
                }
            }
            "findattribval" | "idtopoint" => {
                let (domain, attr, value) = if name == "idtopoint" {
                    (Domain::Point, "id".into(), a[1].clone())
                } else {
                    let domain = match &*a[1].string()? {
                        "point" => Domain::Point,
                        "prim" | "primitive" => Domain::Prim,
                        other => return Err(format!("unknown class \"{other}\" (use \"point\" or \"prim\")")),
                    };
                    (domain, a[2].string()?, a[3].clone())
                };
                let count = domain.count(self.input);
                // Without an id attribute, a point's id is its number.
                if name == "idtopoint" && !self.input.point_attributes.contains_key("id") {
                    let id = value.int()?;
                    return Ok(Int(if id >= 0 && (id as usize) < count { id } else { -1 }));
                }
                Int((0..count).find(|&i| values_equal(&self.input_attr(domain, &attr, i as i32), &value)).map_or(-1, |i| i as i32))
            }
            "primpoint" => Int(usize::try_from(a[1].int()?)
                .ok()
                .and_then(|p| self.polys.get(p))
                .map(|run| crate::poly::points(self.input, run))
                .and_then(|pts| usize::try_from(a[2].int().ok()?).ok().and_then(|v| pts.get(v).copied()))
                .map_or(-1, |p| p as i32)),
            "primvertexcount" => Int(usize::try_from(a[1].int()?).ok().and_then(|p| self.polys.get(p)).map_or(0, |run| crate::poly::points(self.input, run).len() as i32)),
            "vertexpoint" => Int(usize::try_from(a[1].int()?).ok().and_then(|v| self.input.indices.get(v / 3).map(|t| t[v % 3])).map_or(-1, |p| p as i32)),
            "vertexprim" => Int(usize::try_from(a[1].int()?).ok().and_then(|v| self.poly_of.get(v / 3)).map_or(-1, |&p| p as i32)),
            "primintrinsic" => {
                let which = a[1].string()?;
                let Some(run) = usize::try_from(a[2].int()?).ok().and_then(|p| self.polys.get(p)).cloned() else {
                    return Ok(Float(0.0));
                };
                let v = |i: usize| self.input.vertices[i];
                match &*which {
                    "measuredarea" => Float(self.input.indices[run.clone()].iter().map(|t| v(t[1]).sub(v(t[0])).cross(v(t[2]).sub(v(t[0]))).length() * 0.5).sum()),
                    "measuredperimeter" => {
                        let pts = crate::poly::points(self.input, &run);
                        Float((0..pts.len()).map(|i| v(pts[(i + 1) % pts.len()]).sub(v(pts[i])).length()).sum())
                    }
                    other => return Err(format!("unsupported intrinsic \"{other}\" (measuredarea, measuredperimeter)")),
                }
            }
            "volumesample" | "volumegradient" | "volumeindex" | "volumepostoindex" | "volumeindextopos" => {
                let field_name = a[1].string()?;
                let input = self.input;
                if !self.fields.iter().any(|(n, _)| **n == *field_name) {
                    let field = crate::fields::Field::from_mesh(input, &field_name).map_err(|e| format!("{name}(): {e}"))?;
                    self.fields.push((field_name.to_string(), field));
                }
                let field = &self.fields.iter().find(|(n, _)| **n == *field_name).expect("just added").1;
                let v = vec3(a[2].vec()?);
                match name {
                    "volumesample" => Float(field.sample(v)),
                    "volumegradient" => V(arr3(field.gradient(v))),
                    "volumeindex" => Float(field.at_index([v.x.round() as i64, v.y.round() as i64, v.z.round() as i64])),
                    "volumepostoindex" => V(arr3(field.pos_to_index(v))),
                    _ => V(arr3(field.index_to_pos(v))),
                }
            }
            "nearpoint" => {
                let p = vec3(a[1].vec()?);
                let max = a.get(2).map(Value::num).transpose()?.unwrap_or(f32::INFINITY);
                let input = self.input;
                Int(self.grid().nearest(&input.vertices, p, max).map_or(-1, |(i, _)| i as i32))
            }
            "nearpoints" => {
                let p = vec3(a[1].vec()?);
                let max = a[2].num()?;
                let limit = a.get(3).map(Value::int).transpose()?.map_or(usize::MAX, |n| n.max(0) as usize);
                let input = self.input;
                int_array(self.grid().within(&input.vertices, p, max, limit).into_iter().map(|(i, _)| i))
            }
            "pcfind" | "pcopen" => {
                let channel = a[1].string()?;
                if &*channel != "P" {
                    return Err(format!("only the \"P\" channel is supported, got \"{channel}\""));
                }
                let p = vec3(a[2].vec()?);
                let radius = a[3].num()?.max(0.0);
                let limit = a[4].int()?.max(0) as usize;
                let input = self.input;
                let found = self.grid().within(&input.vertices, p, radius, limit);
                if name == "pcfind" {
                    int_array(found.into_iter().map(|(i, _)| i))
                } else {
                    if self.pc_elem != self.elem {
                        self.pc_handles.clear();
                        self.pc_elem = self.elem;
                    }
                    self.pc_handles.push(PcHandle { found, radius, next: 0, current: None });
                    Int(self.pc_handles.len() as i32 - 1)
                }
            }
            "pcnumfound" => Int(self.pc_handle(&a[0])?.found.len() as i32),
            "pcclose" => Int(1),
            "pciterate" => {
                let handle = self.pc_handle_mut(&a[0])?;
                if handle.next < handle.found.len() {
                    handle.current = Some(handle.next);
                    handle.next += 1;
                    Int(1)
                } else {
                    handle.current = None;
                    Int(0)
                }
            }
            "pcfilter" => {
                let attr = a[1].string()?;
                let (found, radius) = {
                    let handle = self.pc_handle(&a[0])?;
                    (handle.found.clone(), handle.radius)
                };
                if found.is_empty() {
                    return Ok(Float(0.0));
                }
                // Closer points count more: w = 1 - (d / radius)^2, equal weights when unbounded.
                let weights: Vec<f32> = found
                    .iter()
                    .map(|(_, d)| if radius.is_finite() && radius > 0.0 { (1.0 - (d / radius).powi(2)).max(1e-6) } else { 1.0 })
                    .collect();
                let total: f32 = weights.iter().sum();
                let values: Vec<Value> = found.iter().map(|(pt, _)| self.input_attr(Domain::Point, &attr, *pt as i32)).collect();
                if values.iter().all(Value::is_vector_like) && values.iter().any(|v| matches!(v, V(_))) {
                    let mut sum = [0.0f32; 3];
                    for (v, w) in values.iter().zip(&weights) {
                        let v = v.vec()?;
                        for c in 0..3 {
                            sum[c] += v[c] * w;
                        }
                    }
                    V(sum.map(|c| c / total))
                } else {
                    let mut sum = 0.0;
                    for (v, w) in values.iter().zip(&weights) {
                        sum += v.num()? * w;
                    }
                    Float(sum / total)
                }
            }
            "xyzdist" | "intersect" | "pcimport" | "translate" | "rotate" | "scale" | "setcomp" => self.builtin_with_outputs(name, a)?.0,
            "minpos" => match closest_on_surface(self.input, vec3(a[1].vec()?)) {
                Some(hit) => V(arr3(hit.position)),
                None => V(a[1].vec()?),
            },
            "curlnoise" => {
                let p = a[0].vec()?;
                // Curl of a vector potential made of three decorrelated Perlin fields.
                let e = 1e-2;
                let field = |c: u32, q: [f32; 3]| perlin3(q, 17 + c);
                let d = |c: u32, axis: usize| {
                    let (mut hi, mut lo) = (p, p);
                    hi[axis] += e;
                    lo[axis] -= e;
                    (field(c, hi) - field(c, lo)) / (2.0 * e)
                };
                V([d(2, 1) - d(1, 2), d(0, 2) - d(2, 0), d(1, 0) - d(0, 1)])
            }
            "sample_direction_uniform" => {
                let u = a[0].vec()?;
                let z = 1.0 - 2.0 * u[1].clamp(0.0, 1.0);
                let r = (1.0 - z * z).max(0.0).sqrt();
                let phi = u[0] * std::f32::consts::TAU;
                V([r * phi.cos(), r * phi.sin(), z])
            }
            "sample_sphere_uniform" => {
                let u = a[0].vec()?;
                let z = 1.0 - 2.0 * u[1].clamp(0.0, 1.0);
                let r = (1.0 - z * z).max(0.0).sqrt();
                let phi = u[0] * std::f32::consts::TAU;
                let scale = u[2].clamp(0.0, 1.0).cbrt();
                V([r * phi.cos() * scale, r * phi.sin() * scale, z * scale])
            }
            "select" => {
                if a[0].truthy() {
                    a[1].clone()
                } else {
                    a[2].clone()
                }
            }
            "slice" => {
                let step = a.get(3).map(Value::int).transpose()?.unwrap_or(1);
                slice_value(&a[0], Some(a[1].int()?), Some(a[2].int()?), step)?
            }
            "reorder" => {
                let items = a[0].items()?;
                let order = a[1].items()?;
                let mut out = Vec::with_capacity(order.len());
                for i in order.iter() {
                    let i = i.int()?;
                    out.push(normalize_index(i, items.len()).map(|i| items[i].clone()).ok_or_else(|| format!("index {i} is out of range"))?);
                }
                Value::Arr(Rc::new(out))
            }
            "uniquevals" => {
                let items = a[0].items()?;
                let order = sort_values(&items)?;
                let mut out: Vec<Value> = Vec::new();
                for i in order {
                    if !out.last().is_some_and(|last| values_equal(last, &items[i])) {
                        out.push(items[i].clone());
                    }
                }
                Value::Arr(Rc::new(out))
            }
            "neighbours" | "neighbourcount" => {
                let pt = a[1].int()?;
                let list = usize::try_from(pt).ok().and_then(|pt| self.neighbour_lists().get(pt).cloned()).unwrap_or_default();
                if name == "neighbourcount" { Int(list.len() as i32) } else { int_array(list) }
            }
            "primpoints" => match usize::try_from(a[1].int()?).ok().and_then(|p| self.polys.get(p)) {
                Some(run) => int_array(crate::poly::points(self.input, run)),
                None => int_array([]),
            },
            "pointprims" => {
                let pt = a[1].int()?;
                let tris = usize::try_from(pt).ok().and_then(|pt| self.point_prim_lists().get(pt).cloned()).unwrap_or_default();
                let mut polys: Vec<usize> = tris.iter().map(|&t| self.poly_of[t]).collect();
                polys.dedup();
                int_array(polys)
            }
            "getbbox_min" => V(arr3(self.bounds().0)),
            "getbbox_max" => V(arr3(self.bounds().1)),
            "getbbox_center" => {
                let (lo, hi) = self.bounds();
                V(arr3(lo.add(hi).scale(0.5)))
            }
            "getbbox_size" => {
                let (lo, hi) = self.bounds();
                V(arr3(hi.sub(lo)))
            }
            "relpointbbox" => {
                let (lo, hi) = self.bounds();
                let p = a[1].vec()?;
                let rel = |c: usize, l: f32, h: f32| if (h - l).abs() < 1e-12 { 0.0 } else { (p[c] - l) / (h - l) };
                V([rel(0, lo.x, hi.x), rel(1, lo.y, hi.y), rel(2, lo.z, hi.z)])
            }
            _ => return Err(format!("internal: built-in '{name}' is not implemented")),
        })
    }
}

fn store_local(fr: &mut Frame, slot: usize, value: Value) -> R<Value> {
    let value = match fr.types[slot] {
        Some(ty) => value.convert(ty)?,
        None => value,
    };
    fr.vals[slot] = value.clone();
    Ok(value)
}

// Grows every attribute of `domain` by `added` zeroed elements.
fn grow_attributes(mesh: &mut Mesh, domain: Domain, old_count: usize, added: usize) {
    if old_count == 0 || added == 0 {
        return;
    }
    for (key, vals) in mesh.point_attributes.iter_mut() {
        if Domain::of_key(key) == domain {
            let comps = vals.len() / old_count;
            vals.resize(vals.len() + comps * added, 0.0);
        }
    }
}

// Keeps the elements of `domain` whose `keep` flag is set, in every attribute of that domain.
fn compact_attributes(mesh: &mut Mesh, domain: Domain, keep: &[bool]) {
    let count = keep.len();
    if count == 0 {
        return;
    }
    for (key, vals) in mesh.point_attributes.iter_mut() {
        if Domain::of_key(key) != domain {
            continue;
        }
        let comps = vals.len() / count;
        if comps == 0 {
            continue;
        }
        let kept: Vec<f32> = vals.chunks(comps).zip(keep).filter(|(_, k)| **k).flat_map(|(chunk, _)| chunk.iter().copied()).collect();
        *vals = kept;
    }
}

fn apply_pending(mesh: &mut Mesh, pending: Pending, log: &mut Vec<String>) -> R<()> {
    let Pending { add_points, add_prims, remove_points, remove_prims, orphan_candidates, writes } = pending;

    let old_points = mesh.vertices.len();
    grow_attributes(mesh, Domain::Point, old_points, add_points.len());
    mesh.vertices.extend(add_points.into_iter().map(vec3));

    let point_count = mesh.vertices.len();
    let (valid, invalid): (Vec<Vec<usize>>, Vec<Vec<usize>>) = add_prims.into_iter().partition(|pts| pts.len() >= 3 && pts.iter().all(|&p| p < point_count));
    if !invalid.is_empty() {
        log.push(format!("Warning: skipped {} primitive(s) with fewer than 3 points or point numbers that do not exist", invalid.len()));
    }
    let old_prims = mesh.indices.len();
    let added: usize = valid.iter().map(|pts| pts.len() - 2).sum();
    grow_attributes(mesh, Domain::Prim, old_prims, added);
    grow_attributes(mesh, Domain::Vertex, old_prims * 3, added * 3);
    for pts in &valid {
        mesh.indices.extend((1..pts.len() - 1).map(|i| [pts[0], pts[i], pts[i + 1]]));
    }
    crate::poly::tag(mesh, old_prims, valid.iter().map(|pts| pts.len() - 2));

    for (domain, name, index, value, mode) in writes {
        let count = domain.count(mesh);
        if index >= count {
            continue;
        }
        let key = format!("{}{name}", domain.prefix());
        let current = || {
            if domain == Domain::Point && name == "P" {
                Value::Vec(arr3(mesh.vertices[index]))
            } else {
                read_storage(mesh, &key, index, count, known_hint(&name))
            }
        };
        let value = match mode {
            WriteMode::Set => value,
            WriteMode::Add => binop(BinOp::Add, current(), value)?,
            WriteMode::Mult => binop(BinOp::Mul, current(), value)?,
            WriteMode::Min => pick_extreme(current(), value, true)?,
            WriteMode::Max => pick_extreme(current(), value, false)?,
        };
        if domain == Domain::Point && name == "P" {
            mesh.vertices[index] = vec3(value.vec()?);
        } else {
            write_storage(mesh, &key, index, count, &value, known_hint(&name), &name)?;
        }
    }

    if remove_points.is_empty() && remove_prims.is_empty() {
        return Ok(());
    }
    let point_count = mesh.vertices.len();
    let mut keep_point: Vec<bool> = (0..point_count).map(|i| !remove_points.contains(&i)).collect();
    let keep_prim: Vec<bool> = mesh
        .indices
        .iter()
        .enumerate()
        .map(|(t, tri)| !remove_prims.contains(&t) && tri.iter().all(|&p| keep_point[p]))
        .collect();
    if !orphan_candidates.is_empty() {
        let mut used = vec![false; point_count];
        for (tri, _) in mesh.indices.iter().zip(&keep_prim).filter(|(_, k)| **k) {
            tri.iter().for_each(|&p| used[p] = true);
        }
        for p in orphan_candidates {
            if p < point_count && !used[p] {
                keep_point[p] = false;
            }
        }
    }
    let mut remap = vec![usize::MAX; point_count];
    let mut next = 0;
    for (i, keep) in keep_point.iter().enumerate() {
        if *keep {
            remap[i] = next;
            next += 1;
        }
    }
    let keep_vertex: Vec<bool> = keep_prim.iter().flat_map(|k| [*k; 3]).collect();
    compact_attributes(mesh, Domain::Point, &keep_point);
    compact_attributes(mesh, Domain::Prim, &keep_prim);
    compact_attributes(mesh, Domain::Vertex, &keep_vertex);
    mesh.vertices = mesh.vertices.iter().zip(&keep_point).filter(|(_, k)| **k).map(|(v, _)| *v).collect();
    mesh.indices = mesh.indices.iter().zip(&keep_prim).filter(|(_, k)| **k).map(|(tri, _)| tri.map(|p| remap[p])).collect();
    Ok(())
}

// ============================================================================
// Wrangle entry point and per-node reports
// ============================================================================

#[derive(Clone, Debug, Default)]
pub struct VexReport {
    pub error: Option<String>,
    pub output: Vec<String>,
    pub elements: usize,
    pub millis: f32,
}

fn reports() -> &'static Mutex<HashMap<u64, VexReport>> {
    static REPORTS: OnceLock<Mutex<HashMap<u64, VexReport>>> = OnceLock::new();
    REPORTS.get_or_init(Default::default)
}

fn report_key(code: &str, domain: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    code.hash(&mut hasher);
    domain.hash(&mut hasher);
    hasher.finish()
}

// The result of the latest cook of this code, for the node's parameter panel.
pub fn last_report(code: &str, domain: usize) -> Option<VexReport> {
    reports().lock().ok()?.get(&report_key(code, domain)).cloned()
}

// Compile errors only, for code that has not been cooked yet.
pub fn check(code: &str) -> Result<(), String> {
    compile(code).map(|_| ()).map_err(|e| e.to_string())
}

// Runs `code` over every element of `domain` (0 detail, 1 primitives, 2 points, 3 vertices).
// On any error the input geometry is returned unchanged and the error is reported.
// `time_inc` is @TimeInc: seconds per frame, or per step inside a SOP Solver.
// A node's ch() parameter values by name (float, int and vector parameters share it).
pub type Channels = std::collections::BTreeMap<String, [f32; 3]>;

// `mask` limits the run to some elements of the domain (a node's group); None runs on all.
// A parameter expression, as typed into a parameter: VEX, with Houdini's
// variables $F (frame, whole), $FF (frame, fractional), $T (seconds) and $FPS.
// Frames count from 1 at time 0.
pub fn eval_expression(expr: &str, time: f32) -> Result<f32, String> {
    let expr = expr.trim();
    if let Ok(v) = expr.parse::<f32>() {
        return Ok(v);
    }
    let fps = crate::sims::SIM_FPS;
    let frame = time * fps + 1.0;
    let source = expr
        .replace("$FPS", &format!("({fps:?})"))
        .replace("$FF", &format!("({frame:?})"))
        .replace("$F", &format!("({:?})", frame.round()))
        .replace("$T", &format!("({time:?})"));
    let code = format!("f@__value = {source};");
    let (mesh, report) = run_masked(Mesh::default(), &code, time, 1.0 / fps, 0, None, None);
    if let Some(error) = report.error {
        return Err(error.replace("f@__value = ", ""));
    }
    mesh.point_attributes.get("detail:__value").and_then(|v| v.first().copied()).filter(|v| v.is_finite()).ok_or_else(|| "the expression has no value".to_string())
}

pub fn run_wrangle(mesh: Mesh, code: &str, time: f32, time_inc: f32, domain_index: usize, mask: Option<&[bool]>, channels: Option<&Channels>) -> Mesh {
    let start = Instant::now();
    let (mesh, mut report) = run_masked(mesh, code, time, time_inc, domain_index, mask, channels);
    report.millis = start.elapsed().as_secs_f32() * 1000.0;
    if let Ok(mut map) = reports().lock() {
        if map.len() > 256 {
            map.clear();
        }
        map.insert(report_key(code, domain_index), report);
    }
    mesh
}

#[cfg(test)]
fn run(mesh: Mesh, code: &str, time: f32, domain_index: usize) -> (Mesh, VexReport) {
    run_timed(mesh, code, time, 1.0 / crate::sims::SIM_FPS, domain_index)
}

fn run_timed(mesh: Mesh, code: &str, time: f32, time_inc: f32, domain_index: usize) -> (Mesh, VexReport) {
    run_masked(mesh, code, time, time_inc, domain_index, None, None)
}

fn run_masked(
    mesh: Mesh,
    code: &str,
    time: f32,
    time_inc: f32,
    domain_index: usize,
    mask: Option<&[bool]>,
    channels: Option<&Channels>,
) -> (Mesh, VexReport) {
    let program = match compile(code) {
        Ok(program) => program,
        Err(error) => return (mesh, VexReport { error: Some(error.to_string()), ..Default::default() }),
    };
    let domain = match domain_index {
        0 => Domain::Detail,
        1 => Domain::Prim,
        2 => Domain::Point,
        _ => Domain::Vertex,
    };
    let normals = mesh.compute_normals();
    let count = domain.count(&mesh);
    let input = mesh.clone();
    let mut rt = Runtime {
        prog: &program,
        input: &input,
        normals: &normals,
        out: mesh,
        domain,
        count,
        keys: program.attrs.iter().map(|a| format!("{}{}", domain.prefix(), a.name)).collect(),
        elem: 0,
        time,
        time_inc,
        pending: Pending::default(),
        steps: 0,
        depth: 0,
        log: Vec::new(),
        partial_line: String::new(),
        neighbours: None,
        point_prims: None,
        point_grid: None,
        fields: Vec::new(),
        channels,
        pc_handles: Vec::new(),
        pc_elem: usize::MAX,
        polys: crate::poly::runs(&input),
        poly_of: crate::poly::of_triangles(&input),
        prim: 0,
    };
    // Reading an attribute nothing creates is almost always a typo or an unsupported global.
    let missing: Vec<String> = program
        .attrs
        .iter()
        .zip(&rt.keys)
        .filter(|(attr, key)| attr.kind == AttrKind::Other && !attr.written && !input.point_attributes.contains_key(*key))
        .map(|(attr, _)| format!("Warning: @{} does not exist on the input and is never set, so it reads as 0", attr.name))
        .collect();
    for warning in missing {
        rt.push_log(warning);
    }
    let defaults: Vec<Value> = program.main.slot_types.iter().map(|t| t.map_or(Value::Int(0), Ty::default_value)).collect();
    let element_name = match domain {
        Domain::Detail => "detail",
        Domain::Prim => "primitive",
        Domain::Point => "point",
        Domain::Vertex => "vertex",
    };
    let mut failure = None;
    // Over primitives, each polygon runs once, on its first triangle.
    let elements: Vec<usize> = if domain == Domain::Prim { rt.polys.iter().map(|run| run.start).collect() } else { (0..count).collect() };
    for (number, &elem) in elements.iter().enumerate() {
        if mask.is_some_and(|m| !m.get(elem).copied().unwrap_or(true)) {
            continue;
        }
        rt.elem = elem;
        rt.prim = if domain == Domain::Prim { number } else { 0 };
        let mut frame = Frame { vals: defaults.clone(), types: &program.main.slot_types };
        if let Err(error) = rt.exec_block(&program.main.body, &mut frame) {
            failure = Some(if domain == Domain::Detail { error } else { format!("{error} (at {element_name} {number})") });
            break;
        }
    }
    // What a polygon's run wrote to its first triangle applies to all of its triangles.
    if domain == Domain::Prim && failure.is_none() {
        let written: Vec<String> = program.attrs.iter().zip(&rt.keys).filter(|(a, k)| a.written && k.starts_with("prim:")).map(|(_, k)| k.clone()).collect();
        let tris = rt.out.indices.len();
        for key in written {
            let Some(values) = rt.out.point_attributes.get_mut(&key) else { continue };
            if tris == 0 || values.len() % tris != 0 {
                continue;
            }
            let width = values.len() / tris;
            for run in rt.polys.iter().filter(|r| r.len() > 1) {
                let first: Vec<f32> = values[run.start * width..(run.start + 1) * width].to_vec();
                for t in run.start + 1..run.end {
                    values[t * width..(t + 1) * width].copy_from_slice(&first);
                }
            }
        }
    }
    let count = elements.len();
    if !rt.partial_line.is_empty() {
        let rest = std::mem::take(&mut rt.partial_line);
        rt.push_log(rest);
    }
    let Runtime { mut out, pending, mut log, .. } = rt;
    if failure.is_none() {
        if let Err(error) = apply_pending(&mut out, pending, &mut log) {
            failure = Some(error);
        }
    }
    match failure {
        Some(error) => (input, VexReport { error: Some(error), output: log, elements: count, millis: 0.0 }),
        None => (out, VexReport { error: None, output: log, elements: count, millis: 0.0 }),
    }
}

// ============================================================================
// Examples and reference shown in the node's parameter panel
// ============================================================================

// (menu label, run-over domain, code)
pub const EXAMPLES: &[(&str, usize, &str)] = &[
    ("Wave (points)", 2, "// Offset each point by a travelling sine wave\n@P.y += sin(@P.x * 3.0 + @Time) * 0.2;\n"),
    (
        "Color by height (if / ternary)",
        2,
        "// Blend two colors by height\nfloat h = fit(@P.y, -0.5, 0.5, 0, 1);\nif (h > 0.5) {\n    @Cd = set(1, h, 0.2);\n} else {\n    @Cd = h < 0.1 ? {0, 0, 0.4} : set(0.2, h, 1);\n}\n",
    ),
    (
        "Ripples (for loop + array)",
        2,
        "// Sum ripples from several centres\nvector centres[] = array(set(0, 0, 0), set(1.5, 0, 1), set(-1, 0, -1.5));\nfloat h = 0;\nfor (int i = 0; i < len(centres); i++) {\n    float d = distance(@P, centres[i]);\n    h += sin(d * 8 - @Time * 4) * exp(-d * 1.5) * 0.15;\n}\n@P.y += h;\n",
    ),
    (
        "Smooth (foreach neighbours)",
        2,
        "// Average each point with its connected neighbours\nint nbrs[] = neighbours(0, @ptnum);\nvector sum = @P;\nforeach (int n; nbrs) {\n    sum += point(0, \"P\", n);\n}\n@P = sum / (len(nbrs) + 1);\n",
    ),
    (
        "Delete points (removepoint)",
        2,
        "// Remove points below a wavy cutoff, with their triangles\nfloat cutoff = sin(@P.x * 2 + @Time) * 0.2;\nif (@P.z < cutoff) {\n    removepoint(0, @ptnum);\n}\n",
    ),
    (
        "Noise warp (user function)",
        2,
        "// Functions are defined before the code that uses them\nvector warp(vector p; float amount) {\n    vector n = noise(p * 1.5 + @Time * 0.2);\n    return p + (n - 0.5) * amount;\n}\n\n@P = warp(@P, 0.6);\n@Cd = noise(@P * 2);\n",
    ),
    (
        "Random colors per face (primitives)",
        1,
        "// Run over Primitives: one random color per polygon\n@Cd = rand(@primnum);\n",
    ),
    (
        "Spiral of points (detail, while)",
        0,
        "// Run over Detail: build a spiral of new points\nint i = 0;\nwhile (i < 200) {\n    float a = i * 0.2;\n    addpoint(0, set(cos(a) * a * 0.05, i * 0.01, sin(a) * a * 0.05));\n    i++;\n}\nprintf(\"Added %d points\\n\", i);\n",
    ),
    (
        "Debug output (printf)",
        2,
        "// printf output appears below the code editor\nif (@ptnum < 5) {\n    printf(\"point %d at %g, %d neighbours\\n\", @ptnum, @P, neighbourcount(0, @ptnum));\n}\n",
    ),
];

pub const SYNTAX_HELP: &str = "\
Types      int, float, vector, vector4, matrix3, matrix, string; arrays: float a[] = {1, 2};
           vector4 q = quaternion(radians(45), {0,1,0});  matrix m = ident();  v * m transforms
Attributes @P @N @Cd @pscale (read/write), @ptnum @primnum @vtxnum @numpt @numprim
           @numvtx @elemnum @numelem @Time @TimeInc @Frame (read-only). Typed: f@mass i@id v@vel
           p@orient (vector4) 3@rot (matrix3) 4@xform (matrix); ch(\"amp\") adds a slider
Literals   1  2.5  1e-3  \"text\"  {1, 0, 0}   M_PI  M_E
Operators  + - * / %  == != < <= > >=  && || !  ?:  = += -= *= /= %=  ++ --
           .x .y .z (.r .g .b) on vectors, [i] on arrays/vectors (negative from end),
           a[start:end:step] slices arrays and strings (a[1:3], a[-2:], a[::-1])
Control    if (...) {...} else if (...) {...} else {...}
           for (int i = 0; i < 10; i++) {...}   while (...) {...}   do {...} while (...);
           foreach (vector p; pts) {...}   foreach (int i; vector p; pts) {...}
           break; continue; return;
Functions  float twice(float x) { return x * 2; }   defined before use, at the top level;
           parameters are separated by ';' or ',' and passed by reference;
           overloads (same name, other parameter types) and recursion work
Comments   // line   /* block */
Notes      int / int is integer division (1 / 2 == 0, write 1.0 / 2 for 0.5).
           rand() and noise() return a vector when assigned to a vector.
           point() reads the input geometry; setpointattrib(), addpoint(), addprim(),
           removepoint() and removeprim() take effect after every element has run.";

#[cfg(test)]
mod tests {
    use super::*;

    // A 3×3 grid of points (9 points, 8 triangles) in the XZ plane.
    fn grid() -> Mesh {
        let mut mesh = Mesh::default();
        for z in 0..3 {
            for x in 0..3 {
                mesh.vertices.push(Vec3::new(x as f32 - 1.0, 0.0, z as f32 - 1.0));
            }
        }
        for z in 0..2 {
            for x in 0..2 {
                let i = z * 3 + x;
                mesh.indices.push([i, i + 3, i + 1]);
                mesh.indices.push([i + 1, i + 3, i + 4]);
            }
        }
        mesh
    }

    fn single_point(p: [f32; 3]) -> Mesh {
        Mesh { vertices: vec![Vec3::new(p[0], p[1], p[2])], ..Mesh::default() }
    }

    fn detail(code: &str) -> VexReport {
        let (_, report) = run(grid(), code, 0.0, 0);
        report
    }

    // Runs `code` over the detail and returns its printf output.
    fn output(code: &str) -> String {
        let report = detail(code);
        assert_eq!(report.error, None, "code:\n{code}");
        report.output.join("\n")
    }

    fn error(code: &str) -> String {
        detail(code).error.expect("expected an error")
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(output("printf(\"%d %d %g %d\\n\", 1 + 2 * 3, (1 + 2) * 3, 1.0 / 4, 7 / 2);"), "7 9 0.25 3");
        assert_eq!(output("printf(\"%d %d %d\\n\", 7 % 3, -7 / 2, 10 - 2 - 3);"), "1 -3 5");
        assert_eq!(output("printf(\"%d %d %d %d\\n\", 1 < 2, 2 <= 1, 1 == 1.0, 3 != 3);"), "1 0 1 0");
        assert_eq!(output("printf(\"%d %d %d\\n\", 1 && 0, 0 || 2, !5);"), "0 1 0");
        assert_eq!(output("int x = 5; printf(\"%s\\n\", x > 3 ? \"big\" : \"small\");"), "big");
        assert_eq!(output("printf(\"%g\\n\", {1, 2, 3} * 2 + 1);"), "{3, 5, 7}");
    }

    #[test]
    fn variables_and_types() {
        assert_eq!(output("int i = 2.9; float f = 3; vector v = 1; printf(\"%d %g %g\\n\", i, f, v);"), "2 3 {1, 1, 1}");
        assert_eq!(output("vector v = {1, 2, 3}; v.y = 5; v[2] += 1; printf(\"%g %g\\n\", v, v.x);"), "{1, 5, 4} 1");
        assert_eq!(output("int a = 1, b = a + 1; a += b; a *= 3; printf(\"%d %d\\n\", a, b);"), "9 2");
        assert_eq!(output("int i = 1; int j = i++; int k = ++i; printf(\"%d %d %d\\n\", i, j, k);"), "3 1 3");
        assert_eq!(output("string s = \"ab\" + \"cd\"; printf(\"%s %d\\n\", toupper(s), strlen(s));"), "ABCD 4");
        assert_eq!(output("x = 4; x = x * 2; printf(\"%d\\n\", x);"), "8");
    }

    #[test]
    fn control_flow() {
        assert_eq!(output("int t = 0; for (int i = 0; i < 5; i++) { t += i; } printf(\"%d\\n\", t);"), "10");
        assert_eq!(output("int i = 0; while (1) { i++; if (i >= 4) break; } printf(\"%d\\n\", i);"), "4");
        assert_eq!(output("int i = 10; do { i++; } while (i < 5); printf(\"%d\\n\", i);"), "11");
        assert_eq!(
            output("int odd = 0; for (int i = 0; i < 10; i++) { if (i % 2 == 0) continue; odd++; } printf(\"%d\\n\", odd);"),
            "5"
        );
        assert_eq!(
            output("int g; if (0) g = 1; else if (2 > 1) g = 2; else g = 3; printf(\"%d\\n\", g);"),
            "2"
        );
        assert_eq!(
            output("float a[] = {1, 2, 3}; float s = 0; foreach (int i; float x; a) { s += x * i; } printf(\"%g\\n\", s);"),
            "8"
        );
        assert_eq!(
            output("int n = 0; for (int i = 0; i < 3; i++) for (int j = 0; j < 3; j++) { if (j == 1) break; n++; } printf(\"%d\\n\", n);"),
            "3"
        );
    }

    #[test]
    fn functions_recursion_and_references() {
        assert_eq!(output("int fact(int n) { if (n <= 1) return 1; return n * fact(n - 1); }\nprintf(\"%d\\n\", fact(5));"), "120");
        assert_eq!(output("void bump(float x) { x += 1; }\nfloat v = 1; bump(v); printf(\"%g\\n\", v);"), "2");
        assert_eq!(output("vector mix(vector a, b; float t) { return lerp(a, b, t); }\nprintf(\"%g\\n\", mix({0,0,0}, {2,4,6}, 0.5));"), "{1, 2, 3}");
    }

    #[test]
    fn arrays_and_strings() {
        assert_eq!(output("int a[]; push(a, 3); push(a, 1); push(a, 2); printf(\"%d %d %d\\n\", len(a), a[-1], max(a));"), "3 2 3");
        assert_eq!(output("int a[] = {3, 1, 2}; printf(\"%d %d %d\\n\", sort(a), find(a, 2), sum(a));"), "{1, 2, 3} 2 6");
        assert_eq!(output("float a[] = {1}; a[3] = 5; printf(\"%g %d\\n\", a, pop(a));"), "{1, 0, 0, 5} 5");
        assert_eq!(output("string parts[] = split(\"a,b,c\", \",\"); printf(\"%s %d\\n\", join(parts, \"-\"), len(parts));"), "a-b-c 3");
        assert_eq!(output("printf(\"%05.1f|%-4d|%+d|%s\\n\", 3.14159, 7, 3, sprintf(\"%x\", 255));"), "003.1|7   |+3|ff");
    }

    #[test]
    fn point_attributes_and_legacy_syntax() {
        let (mesh, report) = run(grid(), "@P.y += 1; @Cd = set(@ptnum, 0, 0); f@mass = @ptnum * 0.5;", 0.0, 2);
        assert_eq!(report.error, None);
        assert!(mesh.vertices.iter().all(|v| v.y == 1.0));
        assert_eq!(mesh.point_attributes["Cd"][3 * 4], 4.0);
        assert_eq!(mesh.point_attributes["mass"][4], 2.0);
        // The original syntax without '@' and with old-style statements keeps working.
        let (mesh, report) = run(grid(), "P.y += sin(P.x * 3.0) * 0.2;", 0.0, 2);
        assert_eq!(report.error, None);
        assert!((mesh.vertices[2].y - 3f32.sin() * 0.2).abs() < 1e-6);
    }

    #[test]
    fn primitive_and_detail_domains() {
        let (mesh, report) = run(grid(), "@Cd = {1, 0, 0}; f@area = @primnum;", 0.0, 1);
        assert_eq!(report.error, None);
        assert_eq!(mesh.point_attributes["prim:Cd"].len(), 8 * 3);
        assert_eq!(mesh.point_attributes["prim:area"][7], 7.0);
        let (mesh, report) = run(grid(), "for (int i = 0; i < 5; i++) addpoint(0, set(i, 2, 0));", 0.0, 0);
        assert_eq!(report.error, None);
        assert_eq!(mesh.vertices.len(), 14);
    }

    #[test]
    fn geometry_edits_are_applied_after_the_run() {
        // Removing the centre point removes the 6 triangles that use it.
        let (mesh, report) = run(grid(), "if (@ptnum == 4) removepoint(0, @ptnum);", 0.0, 2);
        assert_eq!(report.error, None);
        assert_eq!((mesh.vertices.len(), mesh.indices.len()), (8, 2));
        assert!(mesh.indices.iter().flatten().all(|&p| p < 8));
        // setpointattrib with "add" accumulates writes from every element.
        let (mesh, _) = run(grid(), "setpointattrib(0, \"hits\", 0, 1, \"add\");", 0.0, 2);
        assert_eq!(mesh.point_attributes["hits"][0], 9.0);
        let (mesh, _) = run(grid(), "int a = addpoint(0, {0, 1, 0}); addprim(0, \"poly\", 0, 1, a);", 0.0, 0);
        assert_eq!((mesh.vertices.len(), mesh.indices.len()), (10, 9));
    }

    #[test]
    fn neighbours_and_input_reads() {
        assert_eq!(output("printf(\"%d %d\\n\", neighbourcount(0, 4), neighbourcount(0, 0));"), "6 2");
        assert_eq!(output("printf(\"%g %d\\n\", point(0, \"P\", 8), nearpoint(0, {0.9, 0, 0.9}));"), "{1, 0, 1} 8");
        assert_eq!(output("printf(\"%g\\n\", getbbox_size(0));"), "{2, 0, 2}");
    }

    #[test]
    fn rand_and_noise_follow_the_assignment_type() {
        let (mesh, report) = run(grid(), "@Cd = rand(@ptnum); f@n = noise(@P * 1.3);", 0.0, 2);
        assert_eq!(report.error, None);
        let cd = &mesh.point_attributes["Cd"];
        assert!(cd[0] != cd[1] || cd[1] != cd[2], "vector rand gives distinct channels");
        assert!(mesh.point_attributes["n"].iter().all(|n| (0.0..=1.0).contains(n)));
    }

    #[test]
    fn compile_errors_have_line_numbers() {
        let e = error("float a = 1\nfloat b = 2;");
        assert!(e.starts_with("line 1") && e.contains("expected ';'"), "{e}");
        assert!(error("int x = 1;\nx = y + 1;").starts_with("line 2, col 5: unknown variable 'y'"));
        assert!(error("foo(1);").contains("unknown function 'foo'"));
        assert!(error("break;").contains("'break' outside a loop"));
        assert!(error("@ptnum = 3;").contains("read-only"));
        assert!(error("float a = dot({1,0,0});").contains("expects 2 argument(s)"));
        assert!(error("if (1) { float a = 1;").contains("missing '}'"));
    }

    #[test]
    fn runtime_errors_stop_and_keep_the_input() {
        let (mesh, report) = run(grid(), "int i = 0;\nwhile (1) { i++; }", 0.0, 2);
        assert!(report.error.unwrap().contains("infinite loop"));
        assert_eq!(mesh.vertices, grid().vertices);
        assert!(error("int f(int n) { return f(n + 1); }\nf(0);").contains("nested function calls"));
        assert!(error("string s = \"a\";\nfloat x = s * 2;").starts_with("line 2"));
        assert!(error("error(\"custom %d\", 4);").contains("custom 4"));
    }

    #[test]
    fn every_listed_builtin_is_implemented() {
        let input = grid();
        let normals = input.compute_normals();
        let program = compile("").unwrap();
        let mut rt = Runtime {
            prog: &program,
            input: &input,
            normals: &normals,
            out: input.clone(),
            domain: Domain::Point,
            count: 9,
            keys: Vec::new(),
            elem: 0,
            time: 0.0,
            time_inc: 1.0 / 24.0,
            pending: Pending::default(),
            steps: 0,
            depth: 0,
            log: Vec::new(),
            partial_line: String::new(),
            neighbours: None,
            point_prims: None,
            point_grid: None,
            fields: Vec::new(),
            channels: None,
            pc_handles: Vec::new(),
            pc_elem: usize::MAX,
            polys: crate::poly::runs(&input),
            poly_of: crate::poly::of_triangles(&input),
            prim: 0,
        };
        for (name, min, ..) in BUILTINS {
            if MUTATING.contains(name) {
                continue;
            }
            let args = vec![Value::Int(0); *min];
            if let Err(e) = rt.builtin(name, &args, false) {
                assert!(!e.contains("not implemented"), "{name}: {e}");
            }
        }
    }

    #[test]
    fn examples_compile_and_run() {
        for (label, domain, code) in EXAMPLES {
            let (_, report) = run(grid(), code, 0.5, *domain);
            assert_eq!(report.error, None, "example '{label}'");
        }
    }

    #[test]
    fn array_slicing() {
        let (mesh, report) = run(
            grid(),
            "int a[] = {0, 1, 2, 3, 4, 5};\n\
             i@n1 = len(a[1:4]); i@first = a[1:4][0];\n\
             i@n2 = len(a[:-2]); i@last = a[-2:][1];\n\
             i@rev = a[::-1][0]; i@even = len(a[::2]);\n\
             i@s = len(slice(a, 0, 6, 3));\n\
             i@u = len(uniquevals({3, 1, 3, 2, 1}));\n\
             int r[] = reorder({10, 20, 30}, {2, 0}); i@r0 = r[0];",
            0.0,
            2,
        );
        assert_eq!(report.error, None);
        let get = |name: &str| mesh.point_attributes[name][0];
        assert_eq!((get("n1"), get("first"), get("n2"), get("last")), (3.0, 1.0, 4.0, 5.0));
        assert_eq!((get("rev"), get("even"), get("s"), get("u"), get("r0")), (5.0, 3.0, 2.0, 3.0, 30.0));
        assert!(error("int a[] = {1};\nint b[] = a[::0];").contains("step cannot be 0"));
        assert_eq!(output("printf(\"%s %s\\n\", \"houdini\"[1:4], \"abc\"[::-1]);"), "oud cba");
    }

    #[test]
    fn point_grid_matches_brute_force() {
        let points: Vec<Vec3> = (0..400u32).map(|i| Vec3::new(hash01(i, 1) * 4.0, hash01(i, 2), hash01(i, 3) * 2.0 - 7.0)).collect();
        let grid = PointGrid::new(&points);
        for q in 0..60u32 {
            let p = Vec3::new(hash01(q, 9) * 6.0 - 1.0, hash01(q, 8) * 3.0 - 1.0, hash01(q, 7) * 4.0 - 9.0);
            for radius in [0.05, 0.3, 1.5, f32::INFINITY] {
                let mut brute = PointGrid::brute(&points, p, radius);
                brute.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap().then(x.0.cmp(&y.0)));
                brute.truncate(25);
                assert_eq!(grid.within(&points, p, radius, 25), brute);
                assert_eq!(grid.nearest(&points, p, radius), brute.first().copied());
            }
        }
    }

    #[test]
    fn point_clouds() {
        // 3x3 grid with spacing 1: the centre point has 4 neighbours at distance 1.
        let (mesh, report) = run(
            grid(),
            "int h = pcopen(0, \"P\", @P, 1.01, 10);\n\
             i@found = pcnumfound(h);\n\
             v@avg = pcfilter(h, \"P\");\n\
             float total = 0; int count = 0;\n\
             while (pciterate(h)) { float d; pcimport(h, \"point.distance\", d); total += d; count++; }\n\
             f@total = total; i@count = count;\n\
             i@n = len(pcfind(0, \"P\", @P, 1.01, 3));",
            0.0,
            2,
        );
        assert_eq!(report.error, None);
        let centre = mesh.vertices.iter().position(|v| v.x.abs() < 1e-6 && v.z.abs() < 1e-6).unwrap();
        let attr = |name: &str| mesh.point_attributes[name][centre];
        assert_eq!((attr("found"), attr("count"), attr("n")), (5.0, 5.0, 3.0));
        assert!((attr("total") - 4.0).abs() < 1e-5);
        let avg = &mesh.point_attributes["avg"][centre * 3..centre * 3 + 3];
        assert!(avg.iter().all(|c| c.abs() < 1e-5), "{avg:?}");
        assert!(error("int h = pcopen(0, \"Cd\", @P, 1, 4);").contains("\"P\" channel"));
    }

    #[test]
    fn surface_queries() {
        let (mesh, report) = run(
            grid(),
            "if (@ptnum == 0) {\n\
               int prim; vector uv;\n\
               f@d = xyzdist(0, set(0.2, 3.0, 0.1), prim, uv); i@prim = prim;\n\
               v@m = minpos(0, set(0.2, 3.0, 0.1));\n\
               vector hit, huv;\n\
               i@hitprim = intersect(0, set(0.3, 5, 0.3), set(0, -10, 0), hit, huv); v@hit = hit;\n\
               i@miss = intersect(0, set(0.3, 5, 0.3), set(0, 1, 0), hit, huv);\n\
             }",
            0.0,
            2,
        );
        assert_eq!(report.error, None);
        let get = |name: &str| mesh.point_attributes[name][0];
        assert!((get("d") - 3.0).abs() < 1e-5);
        assert!(get("prim") >= 0.0);
        let m = &mesh.point_attributes["m"][0..3];
        assert!((m[0] - 0.2).abs() < 1e-5 && m[1].abs() < 1e-5 && (m[2] - 0.1).abs() < 1e-5, "{m:?}");
        assert!(get("hitprim") >= 0.0);
        assert_eq!(get("miss"), -1.0);
        let hit = &mesh.point_attributes["hit"][0..3];
        assert!((hit[0] - 0.3).abs() < 1e-5 && hit[1].abs() < 1e-5 && (hit[2] - 0.3).abs() < 1e-5, "{hit:?}");
        assert!(error("xyzdist(0, {0,0,0}, 1, 2);").contains("must be a variable"));
    }

    #[test]
    fn curl_noise_and_sampling() {
        let (mesh, report) = run(
            grid(),
            "v@c = curlnoise(@P * 0.7 + 0.31);\n\
             v@dir = sample_direction_uniform(set(rand(@ptnum), rand(@ptnum + 7), 0));\n\
             v@ball = sample_sphere_uniform(set(rand(@ptnum), rand(@ptnum + 7), rand(@ptnum + 3)));\n\
             f@sel = select(@ptnum > 4, 1.0, 2.0);",
            0.0,
            2,
        );
        assert_eq!(report.error, None);
        for i in 0..mesh.vertices.len() {
            let v = |name: &str| Vec3::new(mesh.point_attributes[name][i * 3], mesh.point_attributes[name][i * 3 + 1], mesh.point_attributes[name][i * 3 + 2]);
            assert!((v("dir").length() - 1.0).abs() < 1e-4);
            assert!(v("ball").length() <= 1.0 + 1e-5);
            assert!(v("c").length() > 0.0);
        }
        // Divergence-free: numerical divergence of curlnoise is ~0.
        let (curl, e) = (|p: [f32; 3]| {
            let (m, _) = run(single_point(p), "v@c = curlnoise(@P);", 0.0, 2);
            [m.point_attributes["c"][0], m.point_attributes["c"][1], m.point_attributes["c"][2]]
        }, 1e-2);
        let p = [0.37, 1.21, -0.58];
        let mut div = 0.0;
        for axis in 0..3 {
            let (mut hi, mut lo) = (p, p);
            hi[axis] += e;
            lo[axis] -= e;
            div += (curl(hi)[axis] - curl(lo)[axis]) / (2.0 * e);
        }
        assert!(div.abs() < 0.05, "divergence {div}");
        assert_eq!(mesh.point_attributes["sel"][0], 2.0);
        assert_eq!(mesh.point_attributes["sel"][8], 1.0);
    }

    #[test]
    fn matrices_and_transforms() {
        let out = output(
            "matrix m = maketransform({1, 2, 3}, {0, 90, 0}, {2, 2, 2});\n\
             vector p = {1, 0, 0} * m;\n\
             matrix3 r = ident(); rotate(r, radians(90), {0, 1, 0});\n\
             vector q = {1, 0, 0} * r;\n\
             matrix mi = invert(m);\n\
             vector back = p * mi;\n\
             matrix t = ident(); translate(t, {5, 0, 0}); scale(t, {2, 2, 2});\n\
             vector moved = {1, 1, 1} * t;\n\
             printf(\"%.3f %.3f %.3f | %.3f %.3f | %.3f %.3f %.3f | %.2f | %g %g %g\\n\", p.x, p.y, p.z, q.x, q.z, back.x, back.y, back.z, determinant(m), moved.x, moved.y, moved.z);\n\
             matrix3 k = ident(); setcomp(k, 7, 1, 2); printf(\"%g %g %g\\n\", getcomp(k, 1, 2), getcomp(transpose(k), 2, 1), getcomp(k, 0, 0));",
        );
        let lines: Vec<&str> = out.lines().collect();
        // Scale 2, rotate +X to -Z, then translate by (1, 2, 3).
        assert_eq!(lines[0], "1.000 2.000 1.000 | -0.000 -1.000 | 1.000 0.000 0.000 | 8.00 | 12 2 2");
        assert_eq!(lines[1], "7 7 1");
    }

    #[test]
    fn quaternions_and_orient() {
        let out = output(
            "vector4 q = quaternion(radians(90), {0, 1, 0});\n\
             vector v = qrotate(q, {1, 0, 0});\n\
             vector4 qi = qinvert(q); vector b = qrotate(qi, v);\n\
             vector4 h = slerp({0, 0, 0, 1}, q, 0.5);\n\
             vector e = quaterniontoeuler(eulertoquaternion({0.3, -0.7, 1.1}, XFORM_ZXY), XFORM_ZXY);\n\
             vector d = qrotate(dihedral({1, 0, 0}, {0, 0, 1}), {1, 0, 0});\n\
             matrix3 m = qconvert(q); vector mv = {1, 0, 0} * m;\n\
             printf(\"%.3f %.3f | %.3f | %.4f | %.2f %.2f %.2f | %.3f | %.3f %.3f | %g\\n\", v.x, v.z, b.x, length(h), e.x, e.y, e.z, d.z, mv.x, mv.z, q.w > 0.7);",
        );
        assert_eq!(out, "0.000 -1.000 | 1.000 | 1.0000 | 0.30 -0.70 1.10 | 1.000 | -0.000 -1.000 | 1");
        // p@orient and 3@ / 4@ attributes store 4, 9 and 16 floats.
        let (mesh, report) = run(grid(), "p@orient = quaternion(radians(45), {0, 0, 1}); 3@rot = qconvert(@orient); 4@xform = maketransform({0, @ptnum, 0}, {0, 0, 0}, {1, 1, 1}); @P = @P * 4@xform;", 0.0, 2);
        assert_eq!(report.error, None);
        assert_eq!(mesh.point_attributes["orient"].len(), 9 * 4);
        assert_eq!(mesh.point_attributes["rot"].len(), 9 * 9);
        assert_eq!(mesh.point_attributes["xform"].len(), 9 * 16);
        assert!((mesh.vertices[5].y - 5.0).abs() < 1e-5, "{:?}", mesh.vertices[5]);
        // Reading orient back without a prefix knows it's a vector4.
        let (mesh, report) = run(mesh, "@N = qrotate(@orient, {1, 0, 0});", 0.0, 2);
        assert_eq!(report.error, None);
        assert!((mesh.point_attributes["N"][1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        assert!(error("matrix m[];").contains("arrays of matrix"));
        assert!(error("matrix3 m = ident();\nm.x = 1;").contains("setcomp"));
    }

    #[test]
    fn channels_strings_and_maths() {
        let mut channels = Channels::new();
        channels.insert("amp".into(), [2.5, 0.0, 0.0]);
        channels.insert("count".into(), [3.2, 0.0, 0.0]);
        channels.insert("dir".into(), [0.0, 1.0, 0.0]);
        let (mesh, report) = run_masked(grid(), "@P += chv(\"dir\") * ch(\"amp\"); i@n = chi(\"count\"); f@missing = chf(\"nope\");", 0.0, 1.0, 2, None, Some(&channels));
        assert_eq!(report.error, None);
        assert_eq!(mesh.vertices[0].y, 2.5);
        assert_eq!((mesh.point_attributes["n"][0], mesh.point_attributes["missing"][0]), (3.0, 0.0));
        assert_eq!(channel_refs("ch(\"a\") + chi( \"b\" ) * chv(\"c\") + ch(\"a\")"), vec![("a".to_owned(), 'f'), ("b".to_owned(), 'i'), ("c".to_owned(), 'v')]);
        assert_eq!(
            output("printf(\"[%s] [%s] [%s] %d %d %s %s %d %s\\n\", strip(\"  hi  \"), lstrip(\"xxhix\", \"x\"), rstrip(\"hi\\n\"), match(\"pi*e\", \"piece\"), re_match(\"^[a-z]+[0-9]$\", \"box7\"), re_find(\"[0-9]+\", \"ab123cd\"), re_replace(\"(o+)\", \"[$1]\", \"foo boo\"), len(re_split(\",\\\\s*\", \"a, b,c\")), padzero(4, 42));"),
            "[hi] [hix] [hi] 1 1 123 f[oo] b[oo] 3 0042"
        );
        assert_eq!(
            output("printf(\"%g %g %g %g %g %g %d\\n\", spline(\"linear\", 0.25, 0, 10, 20), spline(\"constant\", 0.6, 1, 2, 3), spline(\"catrom\", 0.5, 0, 1, 2, 3), cbrt(27), invlerp(10, 20, 15), select(1, 4, 5), isnan(0));"),
            "5 2 1.5 3 0.5 4 0"
        );
        let out = output("float a = wnoise({0.3, 0.4, 0.5}); float b = wnoise({7.3, 1.4, 2.5}); float c = anoise({1.2, 0.4, 3.0}); printf(\"%d %d %d\\n\", a >= 0 && a < 1.8, abs(a - b) > 0.0001, c >= 0 && c <= 1);");
        assert_eq!(out, "1 1 1");
        assert!(error("float x = spline(\"bezier\", 0.5, 1, 2);").contains("unknown basis"));
        assert!(error("int x = re_match(\"(\", \"a\");").contains("regular expression"));
    }

    #[test]
    fn primitive_and_vertex_queries() {
        let out = output(
            "vector m = primuv(0, \"P\", 0, {0.5, 0.5, 0});\n\
             printf(\"%g %g %g | %d %d %d %d %d | %d %d | %g %.4f\\n\", m.x, m.y, m.z, primpoint(0, 0, 1), primvertexcount(0, 0), vertexpoint(0, 4), vertexprim(0, 4), primpoint(0, 99, 0), idtopoint(0, 5), findattribval(0, \"point\", \"P\", {1, 0, 1}), primintrinsic(0, \"measuredarea\", 0), primintrinsic(0, \"measuredperimeter\", 0));",
        );
        // Grid triangle 0 is points (0, 3, 1): halfway between 3 and 1.
        assert_eq!(out, "-0.5 0 -0.5 | 3 3 3 1 -1 | 5 8 | 0.5 3.4142");
    }

    #[test]
    fn time_globals_and_missing_attribute_warnings() {
        let (mesh, report) = run_timed(grid(), "@P.y += 2.0 * @TimeInc; f@f = @Frame;", 1.0, 0.25, 2);
        assert_eq!(report.error, None);
        assert_eq!(mesh.vertices[0].y, 0.5);
        assert_eq!(mesh.point_attributes["f"][0], 25.0);
        assert!(report.output.is_empty(), "{:?}", report.output);
        let (_, report) = run(grid(), "@P.y += @speedd;", 0.0, 2);
        assert!(report.output.iter().any(|l| l.contains("@speedd does not exist")), "{:?}", report.output);
        assert!(error("@TimeInc = 1;").contains("read-only"));
    }

}

#[cfg(test)]
mod function_tests {
    use super::*;

    fn out(code: &str) -> String {
        let (_, report) = run(Mesh { vertices: vec![Vec3::new(0.0, 0.0, 0.0)], ..Mesh::default() }, code, 0.0, 0);
        format!("{}{}", report.output.join(""), report.error.map(|e| format!("ERROR {e}")).unwrap_or_default()).trim().to_owned()
    }

    #[test]
    fn user_functions_cover_vex_features() {
        let cases = [
            ("float sq(float x) { return x * x; } printf('%g', sq(3));", "9"),
            ("void bump(float x) { x += 1; } float a = 1; bump(a); printf('%g', a);", "2"),
            ("int fact(int n) { if (n <= 1) return 1; return n * fact(n - 1); } printf('%d', fact(5));", "120"),
            ("function float twice(float x) { return 2 * x; } printf('%g', twice(4));", "8"),
            ("float total(float a[]) { float s = 0; foreach (float v; a) s += v; return s; } float xs[] = {1,2,3}; printf('%g', total(xs));", "6"),
            ("vector up(float s) { return set(0, s, 0); } printf('%g', up(2).y);", "2"),
            // Overloads, chosen by argument types (exact types first, then int -> float).
            ("float f(float x) { return 1; } float f(vector v) { return 2; } float f(string s) { return 3; } printf('%g %g %g', f(1.0), f({1,2,3}), f('a'));", "1 2 3"),
            ("float g(int x) { return 10; } float g(float x) { return 20; } printf('%g %g', g(1), g(1.5));", "10 20"),
            ("float h(float x) { return x; } float h(float x, float y) { return x + y; } printf('%g %g', h(2), h(2, 3));", "2 5"),
            ("float k(vector v) { return v.x; } float k(float a[]) { return len(a); } printf('%g %g', k({4,0,0}), k(array(1.0, 2.0)));", "4 2"),
            ("float m(float x) { return x; } float m(float y) { return y; }", "ERROR"),
            ("float n(vector v) { return 1; } float n(vector4 q) { return 2; } printf('%g', n('text'));", "ERROR"),
        ];
        for (code, expected) in cases {
            let got = out(code);
            assert!(got == expected || (expected == "ERROR" && got.contains("ERROR")), "{code}\n  got: {got}");
        }
    }
}
