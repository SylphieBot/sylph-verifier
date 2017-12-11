use errors::*;
use nom::IResult;
use roblox::{api, RobloxUserID};
use std::collections::{HashSet, HashMap};
use std::panic;
use std::ptr;
use std::process::abort;

// TODO: Review and possibly rewrite this module to deal with malicious role conditions.
// TODO: Main risk is DoS via stack overflow.

const DEFAULT_CONDITION_STRS: &'static [(&'static str, &'static str)] = &[
    ("Verified", "true"),
    ("FormerBC", "builtin_role(NotBC) and badge(Welcome To The Club)"),
    ("NotBC", "not builtin_role(BC) and not builtin_role(TBC) and not builtin_role(OBC)"),
    ("BC", "badge(Builders Club)"),
    ("TBC", "badge(Turbo Builders Club)"),
    ("OBC", "badge(Outrageous Builders Club)"),
    ("DevForum", "dev_trust_level(2+)"),
    ("RobloxAdmin", "badge(Administrator) or group(1200769)"),
    // TODO: Allow using rank names
    ("FormerAccelerator", "group(2868472, 6)"),
    ("FormerIncubator", "group(2868472, 8)"),
    ("FormerIntern", "group(2868472, 10)"),
    ("Accelerator", "group(2868472, 106)"),
    ("Incubator", "group(2868472, 108)"),
    ("Intern", "group(2868472, 110)"),
];
lazy_static!(
    static ref DEFAULT_CONDITIONS: HashMap<&'static str, VerificationRule<'static>> = {
        let mut map = HashMap::new();
        for &(name, rule) in DEFAULT_CONDITION_STRS {
            map.insert(name, VerificationRule::from_str(rule).unwrap());
        }
        map
    };
);

fn replace_map<T, F: FnOnce(T) -> T>(t: &mut T, f: F) {
    unsafe {
        let t = t as *mut T;
        ptr::write(t, match panic::catch_unwind(panic::AssertUnwindSafe(|| f(ptr::read(t)))) {
            Ok(t) => t,
            Err(_) => abort(),
        })
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
enum RuleAST<'a> {
    Literal(bool),
    Term(&'a str, &'a str),
    Or(Box<RuleAST<'a>>, Box<RuleAST<'a>>),
    And(Box<RuleAST<'a>>, Box<RuleAST<'a>>),
    Not(Box<RuleAST<'a>>),
}
impl <'a> RuleAST<'a> {
    fn simplify(mut self) -> Self {
        // Simplify child terms first.
        match self {
            RuleAST::And(box ref mut a, box ref mut b) => {
                replace_map(a, |x| x.simplify());
                replace_map(b, |x| x.simplify());
            }
            RuleAST::Or(box ref mut a, box ref mut b) => {
                replace_map(a, |x| x.simplify());
                replace_map(b, |x| x.simplify());
            }
            RuleAST::Not(box ref mut a) => {
                replace_map(a, |x| x.simplify());
            }
            _ => { }
        }

        // Peephole optimizations
        match self {
            // Constant propergation
            RuleAST::And(box RuleAST::Literal(a), box RuleAST::Literal(b)) =>
                RuleAST::Literal(a || b),
            RuleAST::Or(box RuleAST::Literal(a), box RuleAST::Literal(b)) =>
                RuleAST::Literal(a || b),
            RuleAST::Not(box RuleAST::Literal(a)) =>
                RuleAST::Literal(!a),
            RuleAST::And(box RuleAST::Literal(false), _) |
            RuleAST::And(_, box RuleAST::Literal(false)) =>
                RuleAST::Literal(false),
            RuleAST::Or(box RuleAST::Literal(true), _) |
            RuleAST::Or(_, box RuleAST::Literal(true)) =>
                RuleAST::Literal(true),
            // Remove redundant expressions
            RuleAST::And(box RuleAST::Literal(true), box a) |
            RuleAST::And(box a, box RuleAST::Literal(true)) |
            RuleAST::Or(box RuleAST::Literal(false), box a) |
            RuleAST::Or(box a, box RuleAST::Literal(false)) |
            RuleAST::Not(box RuleAST::Not(box a)) =>
                a,
            // De Morgan's laws
            RuleAST::And(box RuleAST::Not(a), box RuleAST::Not(b)) =>
                RuleAST::Not(box RuleAST::Or(a, b)),
            RuleAST::Or(box RuleAST::Not(a), box RuleAST::Not(b)) =>
                RuleAST::Not(box RuleAST::And(a, b)),
            x => x,
        }
    }
}

named!(ident(&str) -> &str, re_find_static!("^[_A-Za-z][_A-Za-z0-9]*"));
named!(term_contents(&str) -> &str, re_find_static!("^[^()]*"));
macro_rules! ident {
    ($i:expr, $a:expr) => { map_opt!($i, ident, |x| if x == $a { Some(x) } else { None }) }
}

named!(expr_0(&str) -> RuleAST, ws!(alt_complete!(
    delimited!(tag_s!("("), expr, tag_s!(")")) |
    map!(ident!("true"), |_| RuleAST::Literal(true)) |
    map!(ident!("false"), |_| RuleAST::Literal(false)) |
    map!(ws!(tuple!(ident, delimited!(tag_s!("("), term_contents, tag_s!(")")))),
         |t| RuleAST::Term(t.0, t.1.trim()))
)));
named!(expr_1(&str) -> RuleAST, ws!(alt_complete!(
    map!(preceded!(ident!("not"), expr_1), |t| RuleAST::Not(box t)) |
    expr_0
)));
named!(expr_2(&str) -> RuleAST, ws!(alt_complete!(
    map!(tuple!(expr_1, preceded!(ident!("or"), expr_2)),
         |t| RuleAST::Or(box t.0, box t.1)) |
    expr_1
)));
named!(expr(&str) -> RuleAST, ws!(alt_complete!(
    map!(tuple!(expr_2, preceded!(ident!("and"), expr)),
         |t| RuleAST::And(box t.0, box t.1)) |
    expr_2
)));

fn check_iresult<T>(result: IResult<&str, T>) -> Result<T> {
    match result {
        IResult::Done(rest, val) => if !rest.is_empty() {
            bail!("error parsing expression at: {}", rest)
        } else {
            Ok(val)
        },
        IResult::Error(e) => bail!("error parsing expression at: {}", e),
        IResult::Incomplete(_) => bail!("unexpected end of expression"),
    }
}
pub struct VerificationRule<'a>(RuleAST<'a>);
impl <'a> VerificationRule<'a> {
    pub fn from_str(rule: &str) -> Result<VerificationRule> {
        ensure!(rule.len() < 500, "Verification rule cannot be over 500 characters.");
        Ok(VerificationRule(check_iresult(expr(rule))?.simplify()))
    }
}

#[derive(Copy, Clone, Debug)]
enum Comparison {
    Equals(u32),
    GreaterOrEqual(u32),
    LessOrEqual(u32),
}
impl Comparison {
    fn satisifies(&self, val: u32) -> bool {
        match self {
            &Comparison::Equals(i) => val == i,
            &Comparison::GreaterOrEqual(i) => val >= i,
            &Comparison::LessOrEqual(i) => val <= i,
        }
    }
}
named!(parse_u32(&str) -> u32, map_res!(re_find_static!("^[0-9]+"), |x : &str| x.parse::<u32>()));
named!(parse_condition(&str) -> Comparison, ws!(alt_complete!(
    map!(ws!(terminated!(parse_u32, tag_s!("+"))), |t| Comparison::GreaterOrEqual(t)) |
    map!(ws!(terminated!(parse_u32, tag_s!("+"))), |t| Comparison::LessOrEqual(t)) |
    map!(parse_u32, |t| Comparison::Equals(t))
)));

#[derive(Clone, Debug)]
enum RuleOp {
    Literal(bool),
    CheckBadge(String),
    CheckPlayerBadge(u64),
    CheckOwnsAsset(u64),
    CheckInGroup(u64, Option<Comparison>),
    CheckDevTrustLevel(Comparison),
    Not(usize),
    Or(usize, usize),
    And(usize, usize),
}

struct CompileContext<'a> {
    ops: Vec<RuleOp>, map: HashMap<&'a RuleAST<'a>, usize>, processing_roles: HashSet<&'a str>,
}
impl <'a> CompileContext<'a> {
    fn new() -> Self {
        CompileContext {
            ops: Vec::new(), map: HashMap::new(), processing_roles: HashSet::new()
        }
    }

    fn push_op(&mut self, op: RuleOp) -> usize {
        self.ops.push(op);
        self.ops.len() - 1
    }

    fn compile_expr(&mut self, expr: &'a RuleAST<'a>,
                    roles: &'a HashMap<&'a str, VerificationRule<'a>>) -> Result<usize> {
        if let Some(&id) = self.map.get(&expr) {
            Ok(id)
        } else {
            let id = match expr {
                &RuleAST::Literal(b) =>
                    self.push_op(RuleOp::Literal(b)),
                &RuleAST::Not(box ref a) => {
                    let op = RuleOp::Not(self.compile_expr(a, roles)?);
                    self.push_op(op)
                },
                &RuleAST::Or(box ref a, box ref b) => {
                    let op = RuleOp::Or(self.compile_expr(a, roles)?, self.compile_expr(b, roles)?);
                    self.push_op(op)
                }
                &RuleAST::And(box ref a, box ref b) => {
                    let op = RuleOp::And(self.compile_expr(a, roles)?, self.compile_expr(b, roles)?);
                    self.push_op(op)
                }
                &RuleAST::Term("role", role) => {
                    ensure!(!self.processing_roles.contains(&role),
                            "loop while processing role: {}", role);
                    self.processing_roles.insert(role);
                    let ast = roles.get(role).chain_err(|| format!("role not found: {}", role))?;
                    let id = self.compile_expr(&ast.0, roles)?;
                    self.processing_roles.remove(&role);
                    id
                }
                &RuleAST::Term("builtin_role", role) => {
                    let ast = DEFAULT_CONDITIONS.get(role)
                        .chain_err(|| format!("built-in role not found: {}", role))?;
                    self.compile_expr(&ast.0, roles)?
                }
                &RuleAST::Term("badge", badge) =>
                    self.push_op(RuleOp::CheckBadge(badge.to_owned())),
                &RuleAST::Term("player_badge", badge) => {
                    let badge = badge.parse()
                        .chain_err(|| format!("badge id is not a number: {}", badge))?;
                    self.push_op(RuleOp::CheckPlayerBadge(badge))
                }
                &RuleAST::Term("owns_asset", asset) => {
                    let asset = asset.parse()
                        .chain_err(|| format!("asset id is not a number: {}", asset))?;
                    self.push_op(RuleOp::CheckOwnsAsset(asset))
                }
                &RuleAST::Term("dev_trust_level", level) => {
                    let level = check_iresult(parse_condition(level))
                        .chain_err(|| format!("invalid trust level: {}", level))?;
                    self.push_op(RuleOp::CheckDevTrustLevel(level))
                }
                &RuleAST::Term("group", group_def) => {
                    let split: Vec<&str> = group_def.split(",").collect();
                    let group = split[0].trim();
                    let group = group.parse()
                        .chain_err(|| format!("group id is not a number: {}", group))?;
                    if split.len() == 1 {
                        self.push_op(RuleOp::CheckInGroup(group, None))
                    } else if split.len() == 2 {
                        let level = split[1].trim();
                        let level = check_iresult(parse_condition(level))
                            .chain_err(|| format!("invalid group level: {}", level))?;
                        self.push_op(RuleOp::CheckInGroup(group, Some(level)))
                    } else {
                        bail!("too many parameters in group({})", group_def)
                    }
                }
                &RuleAST::Term(func, data) =>
                    bail!("unknown term: {}({})", func, data),
            };
            self.map.insert(expr, id);
            Ok(id)
        }
    }

    fn finish(self) -> Vec<RuleOp> {
        self.ops
    }
}

fn option_cache<T, F>(opt: &mut Option<T>, f: F) -> Result<&T> where F: FnOnce() -> Result<T> {
    if opt.is_none() { *opt = Some(f()?) }
    if let &mut Some(ref t) = opt { Ok(t) } else { unreachable!() }
}

struct VerificationContext<'a> {
    ops: &'a [RuleOp], user_id: RobloxUserID, cache: Vec<Option<bool>>,
    username: Option<String>, dev_trust_level: Option<Option<u32>>,
    badges: Option<HashSet<String>>, groups: Option<HashMap<u64, u32>>,
}
impl <'a> VerificationContext<'a> {
    fn new(ops: &'a [RuleOp], user_id: RobloxUserID) -> VerificationContext {
        VerificationContext {
            ops, user_id, cache: vec![None; ops.len()],
            username: None, dev_trust_level: None, badges: None, groups: None,
        }
    }

    fn raw_username(id: RobloxUserID, username: &mut Option<String>) -> Result<&str> {
        option_cache(username, || id.lookup_username()).map(|x| x.as_ref())
    }
    fn dev_trust_level(&mut self) -> Result<Option<u32>> {
        let id = self.user_id;
        let username = &mut self.username;
        option_cache(&mut self.dev_trust_level,
                     || api::get_dev_trust_level(VerificationContext::raw_username(id, username)?))
            .map(|x| *x)
    }
    fn badges(&mut self) -> Result<&HashSet<String>> {
        let id = self.user_id;
        option_cache(&mut self.badges, || api::get_roblox_badges(id))
    }
    fn groups(&mut self) -> Result<&HashMap<u64, u32>> {
        let id = self.user_id;
        option_cache(&mut self.groups, || api::get_player_groups(id))
    }

    fn eval(&mut self, i: usize) -> Result<bool> {
        match self.cache[i] {
            Some(b) => Ok(b),
            None => {
                let op = &self.ops[i];
                let new = match op {
                    &RuleOp::Literal(b) => b,
                    &RuleOp::CheckBadge(ref name) => self.badges()?.contains(name),
                    &RuleOp::CheckPlayerBadge(id) => api::has_player_badge(self.user_id, id)?,
                    &RuleOp::CheckOwnsAsset(asset) => api::owns_asset(self.user_id, asset)?,
                    &RuleOp::CheckInGroup(group, None) => self.groups()?.contains_key(&group),
                    &RuleOp::CheckInGroup(group, Some(check)) => match self.groups()?.get(&group) {
                        Some(&level) => check.satisifies(level),
                        None => false,
                    },
                    &RuleOp::CheckDevTrustLevel(check) => match self.dev_trust_level()? {
                        Some(level) => check.satisifies(level),
                        None => false,
                    }
                    &RuleOp::Not(a) => !self.eval(a)?,
                    &RuleOp::Or(a, b) => self.eval(a)? || self.eval(b)?,
                    &RuleOp::And(a, b) => self.eval(a)? && self.eval(b)?,
                };
                self.cache[i] = Some(new);
                Ok(new)
            }
        }
    }
}

#[derive(Clone, Debug)]
struct VerificationOutput {
    role: String, entry: usize,
}

#[derive(Clone, Debug)]
pub struct VerificationSet {
    ops: Vec<RuleOp>, outputs: Vec<VerificationOutput>,
}
impl VerificationSet {
    pub fn compile(active_roles: &[&str],
                   roles: HashMap<&str, VerificationRule>) -> Result<VerificationSet> {
        let mut entries = Vec::new();
        for &role in active_roles {
            if roles.contains_key(&role) {
                entries.push((role, RuleAST::Term("role", role)))
            } else if DEFAULT_CONDITIONS.contains_key(&role) {
                entries.push((role, RuleAST::Term("builtin_role", role)))
            } else {
                bail!("role enabled, but definition not found: {}", role)
            }
        }

        let mut context = CompileContext::new();
        let mut outputs = Vec::new();
        for &(role, ref ast) in &entries {
            let entry = context.compile_expr(ast, &roles)?;
            outputs.push(VerificationOutput {
                role: role.to_owned(), entry,
            })
        }
        Ok(VerificationSet {
            ops: context.finish(), outputs,
        })
    }

    // TODO: Calculate maximum web requests and complexity (in case of abuse of a hosted bot)

    pub fn verify<F>(&self, id: RobloxUserID,
                     mut f: F) -> Result<()> where F: FnMut(&str, bool) -> Result<()> {
        let mut context = VerificationContext::new(&self.ops, id);
        for &VerificationOutput { ref role, entry } in &self.outputs {
            f(role, context.eval(entry)?)?
        }
        Ok(())
    }
}