use errors::*;
use nom::IResult;
use roblox::RobloxUserID;
use std::borrow::Cow;
use std::collections::{HashSet, HashMap};

const DEFAULT_CONDITION_STRS: &'static [(&'static str, &'static str)] = &[
    ("FormerBC", "builtin_role(Not_BC) and badge(Welcome To The Club)"),
    ("NotBC", "not builtin_role(BC) and not builtin_role(TBC) and not builtin_role(OBC)"),
    ("BC", "badge(Builders Club)"),
    ("TBC", "badge(Turbo Builders Club)"),
    ("OBC", "badge(Outrageous Builders Club)"),
    ("DevForum", "dev_trust_level(>2)"),
    ("RobloxAdmin", "badge(Administrator) or group(1200769)"),
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

#[derive(Clone, Eq, PartialEq, Debug)]
enum RuleAST<'a> {
    Literal(bool),
    Term(&'a str, &'a str),
    Or(Box<RuleAST<'a>>, Box<RuleAST<'a>>),
    And(Box<RuleAST<'a>>, Box<RuleAST<'a>>),
    Not(Box<RuleAST<'a>>),
}

named!(ident(&str) -> &str, re_find_static!("^[_A-Za-z][_A-Za-z0-9]*"));
named!(term_contents(&str) -> &str, re_find_static!("^[^()]*"));

named!(expr_0(&str) -> RuleAST, ws!(alt_complete!(
    delimited!(tag_s!("("), expr, tag_s!(")")) |
    map!(tag_s!("true"), |_| RuleAST::Literal(true)) |
    map!(tag_s!("false"), |_| RuleAST::Literal(false)) |
    map!(ws!(tuple!(ident, delimited!(tag_s!("("), term_contents, tag_s!(")")))),
         |t| RuleAST::Term(t.0, t.1.trim()))
)));
named!(expr_1(&str) -> RuleAST, ws!(alt_complete!(
    map!(preceded!(tag_s!("not"), expr_1), |t| RuleAST::Not(box t)) |
    expr_0
)));
named!(expr_2(&str) -> RuleAST, ws!(alt_complete!(
    map!(tuple!(expr_1, preceded!(tag_s!("or"), expr_2)),
         |t| RuleAST::Or(box t.0, box t.1)) |
    expr_1
)));
named!(expr(&str) -> RuleAST, ws!(alt_complete!(
    map!(tuple!(expr_2, preceded!(tag_s!("and"), expr)),
         |t| RuleAST::And(box t.0, box t.1)) |
    expr_2
)));
fn parse_rule(rule: &str) -> Result<RuleAST> {
    match expr(rule) {
        IResult::Done(rest, ast) => if !rest.is_empty() {
            bail!("error parsing expression at: {}", rest)
        } else {
            Ok(ast)
        },
        IResult::Error(e) => bail!("error parsing expression at: {}", e),
        IResult::Incomplete(_) => bail!("unexpected end of expression"),
    }
}

pub struct VerificationRule<'a>(RuleAST<'a>);
impl <'a> VerificationRule<'a> {
    pub fn from_str(rule: &str) -> Result<VerificationRule> {
        Ok(VerificationRule(parse_rule(rule)?))
    }
}

#[derive(Clone, Debug)]
enum Comparison {
    NotEquals(u32),
    Equals(u32),
    Greater(u32),
    Less(u32),
    GreaterOrEqual(u32),
    LessOrEqual(u32),
}
impl Comparison {
    fn satisifies(&self, val: u32) -> bool {
        match self {
            &Comparison::NotEquals(i) => val != i,
            &Comparison::Equals(i) => val == i,
            &Comparison::Greater(i) => val > i,
            &Comparison::Less(i) => val < i,
            &Comparison::GreaterOrEqual(i) => val >= i,
            &Comparison::LessOrEqual(i) => val <= i,
        }
    }
}

#[derive(Clone, Debug)]
enum RuleOp {
    Literal(bool),
    CheckBadge(String),
    CheckPlayerBadge(u64),
    CheckHasAsset(u64),
    IsInGroup(u64, Option<Comparison>),
    CheckDevTrustLevel(Comparison),
    Not(usize),
    Or(usize, usize),
    And(usize, usize),
    Output(usize, String),
}

#[derive(Clone, Debug)]
pub struct VerificationSet(Vec<RuleOp>);
impl VerificationSet {
    pub fn compile(active_roles: &[&str],
                   rule_overrides: &[(&str, VerificationRule)]) -> Result<VerificationSet> {
        unimplemented!()
    }
}