use errors::*;
use roblox::{api, RobloxUserID};
use std::collections::{HashSet, HashMap, VecDeque};
use std::fmt;
use std::str::from_utf8;

const DEFAULT_RULE_DEFS: &[(&str, &str)] = &[
    ("Verified", "true"),
    ("Banned", "is_banned()"),
    ("NotBanned", "not is_banned()"),
    ("FormerBC", "builtin_rule(NotBC) and badge(Welcome To The Club)"),
    ("NotBC", "not builtin_rule(BC) and not builtin_rule(TBC) and not builtin_rule(OBC)"),
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
    static ref DEFAULT_RULES: HashMap<&'static str, VerificationRule> = {
        let mut map = HashMap::new();
        for &(rule_name, rule) in DEFAULT_RULE_DEFS {
            map.insert(rule_name, VerificationRule::from_str(rule).unwrap());
        }
        map
    };
);

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum RuleResult {
    True, False, Error,
}

impl From<bool> for RuleResult {
    fn from(b: bool) -> Self {
        if b { RuleResult::True } else { RuleResult::False }
    }
}
impl From<Option<bool>> for RuleResult {
    fn from(b: Option<bool>) -> Self {
        b.map_or(RuleResult::Error, |x| x.into())
    }
}

#[derive(Debug)]
enum Token<'a> {
    Term(&'a str, &'a str), Literal(RuleResult), Not, Or, And, OpenParen, CloseParen,
}
impl <'a> fmt::Display for Token<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Token::Term(start, body)          => write!(f, "{}({})", start, body),
            Token::Literal(RuleResult::True ) => write!(f, "true"),
            Token::Literal(RuleResult::False) => write!(f, "false"),
            Token::Literal(RuleResult::Error) => write!(f, "synthetic_error"),
            Token::Not                        => write!(f, "not"),
            Token::Or                         => write!(f, "or"),
            Token::And                        => write!(f, "and"),
            Token::OpenParen                  => write!(f, "("),
            Token::CloseParen                 => write!(f, ")"),
        }
    }
}

fn expect_char(rule: &[u8], pos: usize, what: &str) -> Result<u8> {
    cmd_ensure!(pos < rule.len(), "Unexpected end of line after {}.", what);
    Ok(rule[pos])
}
fn is_ident_char(c: u8) -> bool {
    c == b'_' || (c >= b'a' && c <= b'z') || (c >= b'A' && c <= b'Z')
}
fn advance_whitespace(rule: &[u8], mut current_pos: usize) -> usize {
    while current_pos < rule.len() {
        match rule[current_pos] {
            b' ' | b'\t' => current_pos += 1,
            _ => break,
        }
    }
    current_pos
}
fn tokenize_rule(rule: &str) -> Result<VecDeque<Token>> {
    cmd_ensure!(rule.is_ascii(), "Rules may only contain ASCII characters.");
    let rule = rule.as_bytes();

    let mut current_pos = 0;
    let mut tokens = VecDeque::new();
    while current_pos < rule.len() {
        current_pos = advance_whitespace(rule, current_pos);
        match rule[current_pos] {
            b'(' => {
                tokens.push_back(Token::OpenParen);
                current_pos += 1;
            }
            b')' => {
                tokens.push_back(Token::CloseParen);
                current_pos += 1;
            }
            c if is_ident_char(c) => {
                let token_start = current_pos;
                current_pos += 1;
                while current_pos < rule.len() && is_ident_char(rule[current_pos]) {
                    current_pos += 1;
                }
                match from_utf8(&rule[token_start..current_pos])? {
                    "true"            => tokens.push_back(Token::Literal(RuleResult::True)),
                    "false"           => tokens.push_back(Token::Literal(RuleResult::False)),
                    "synthetic_error" => tokens.push_back(Token::Literal(RuleResult::Error)),
                    "not"             => tokens.push_back(Token::Not),
                    "or"              => tokens.push_back(Token::Or),
                    "and"             => tokens.push_back(Token::And),
                    term_start => {
                        current_pos = advance_whitespace(rule, current_pos);
                        cmd_ensure!(expect_char(rule, current_pos, "start of term")? == b'(',
                                    "Unexpected character after start of term.");
                        current_pos += 1;
                        let body_start = current_pos;
                        while expect_char(rule, current_pos, "term body")? != b')' {
                            current_pos += 1;
                        }
                        let term_body = from_utf8(&rule[body_start..current_pos])?;
                        tokens.push_back(Token::Term(term_start, term_body.trim()));
                        current_pos += 1;
                    }
                }
            }
            c => cmd_error!("Unexpected character: '{}'", c),
        }
    }
    Ok(tokens)
}

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
enum RuleSource {
    BuiltinRule(String), CustomRule(String),
}

#[derive(Copy, Clone, Debug)]
enum Condition {
    Equals(u32),
    GreaterOrEqual(u32),
    LessOrEqual(u32),
}
impl Condition {
    fn satisifies(&self, val: u32) -> bool {
        match *self {
            Condition::Equals(i) => val == i,
            Condition::GreaterOrEqual(i) => val >= i,
            Condition::LessOrEqual(i) => val <= i,
        }
    }
}
fn parse_condition(condition: &str) -> Result<Condition> {
    if condition.ends_with('+') {
        Ok(Condition::GreaterOrEqual(condition[..condition.len()-1].parse()?))
    } else if condition.ends_with('-') {
        Ok(Condition::LessOrEqual(condition[..condition.len()-1].parse()?))
    } else {
        Ok(Condition::Equals(condition.parse()?))
    }
}

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
enum CompileOperator {
    Any, Or, And, Not, OpenParen,
}

#[derive(Copy, Clone, Debug)]
enum Operator {
    Or, And, Not,
}

#[derive(Clone, Debug)]
enum RuleOp {
    Read(usize),
    Output(Option<usize>, Option<String>),
    Literal(RuleResult),
    StartSkip(RuleResult, usize),
    Operator(Option<usize>, Operator),
    CheckBadge(String),
    CheckPlayerBadge(u64),
    CheckOwnsAsset(u64),
    CheckInGroup(u64, Option<Condition>),
    CheckDevTrustLevel(Condition),
    CheckIsBanned,
}
impl RuleOp {
    pub fn stack_change(&self) -> isize {
        match self {
            RuleOp::Read(_)                    =>  1,
            RuleOp::Output(_, _)               => -1,
            RuleOp::Literal(_)                 =>  1,
            RuleOp::StartSkip(_, _)            =>  0,
            RuleOp::Operator(_, Operator::And) => -1,
            RuleOp::Operator(_, Operator::Or)  => -1,
            RuleOp::Operator(_, Operator::Not) =>  0,
            RuleOp::CheckBadge(_)              =>  1,
            RuleOp::CheckPlayerBadge(_)        =>  1,
            RuleOp::CheckOwnsAsset(_)          =>  1,
            RuleOp::CheckInGroup(_, _)         =>  1,
            RuleOp::CheckDevTrustLevel(_)      =>  1,
            RuleOp::CheckIsBanned              =>  1,
        }
    }
}

fn parse_term(start: &str, body: &str) -> Result<RuleOp> {
    match start {
        "badge" =>
            Ok(RuleOp::CheckBadge(body.to_owned())),
        "player_badge" => {
            let badge = body.parse()
                .to_cmd_err(|| format!("Badge id is not a number: {}", body))?;
            Ok(RuleOp::CheckPlayerBadge(badge))
        }
        "owns_asset" => {
            let asset = body.parse()
                .to_cmd_err(|| format!("Asset id is not a number: {}", body))?;
            Ok(RuleOp::CheckOwnsAsset(asset))
        }
        "dev_trust_level" => {
            let level = parse_condition(body)
                .to_cmd_err(|| format!("Invalid trust level: {}", body))?;
            Ok(RuleOp::CheckDevTrustLevel(level))
        }
        "group" => {
            let split: Vec<&str> = body.split(',').collect();
            let group = split[0].trim();
            let group = group.parse()
                .to_cmd_err(|| format!("Group ID is not a number: {}", group))?;
            if split.len() == 1 {
                Ok(RuleOp::CheckInGroup(group, None))
            } else if split.len() == 2 {
                let level = split[1].trim();
                let level = parse_condition(level)
                    .to_cmd_err(|| format!("Invalid group level: {}", level))?;
                Ok(RuleOp::CheckInGroup(group, Some(level)))
            } else {
                cmd_error!("Too many parameters in group({})", body)
            }
        }
        "is_banned" => {
            ensure!(body == "", "is_banned takes no parameters.");
            Ok(RuleOp::CheckIsBanned)
        }
        _ => cmd_error!("Unknown term {}({})", start, body),
    }
}

struct CompileContext {
    ops: Vec<RuleOp>, op_stack: Vec<(CompileOperator, Option<usize>)>, op_id: usize,
}
impl CompileContext {
    fn new() -> CompileContext {
        CompileContext { ops: Vec::new(), op_stack: Vec::new(), op_id: 0 }
    }
    fn push_term(&mut self, op: RuleOp) {
        self.ops.push(op)
    }
    fn push_op(&mut self, op: CompileOperator) {
        self.op_stack.push((op, None))
    }
    fn push_op_skip(&mut self, skip_cond: RuleResult, op: CompileOperator) {
        self.ops.push(RuleOp::StartSkip(skip_cond, self.op_id));
        self.op_stack.push((op, Some(self.op_id)));
        self.op_id += 1;
    }
    fn pop_op_stack(&mut self, max: CompileOperator) -> Result<()> {
        loop {
            let last = self.op_stack.last().cloned();
            match last {
                Some((op, id)) if op != CompileOperator::OpenParen && op > max =>
                    self.ops.push(RuleOp::Operator(id, match op {
                        CompileOperator::Not => Operator::Not,
                        CompileOperator::And => Operator::And,
                        CompileOperator::Or  => Operator::Or ,
                        op => bail!("Internal error: Invalid bytecode operator: {:?}", op),
                    })),
                _ => break,
            }
            self.op_stack.pop();
        }
        Ok(())
    }
    fn pop_open_paren(&mut self) -> Result<()> {
        if let Some((CompileOperator::OpenParen, _)) = self.op_stack.pop() {
            Ok(())
        } else {
            cmd_error!("Unbalanced parentheses.")
        }
    }
}

fn disasm(asm: &[RuleOp], tab: &str, f: &mut fmt::Formatter) -> fmt::Result {
    if asm.is_empty() {
        writeln!(f, "\tno instructions")?;
    } else {
        for line in asm {
            writeln!(f, "{}{:?}", tab, line)?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct VerificationRule {
    inputs: Vec<RuleSource>, ops: Vec<RuleOp>, op_id_count: usize,
}
impl VerificationRule {
    pub fn from_str(rule: &str) -> Result<VerificationRule> {
        cmd_ensure!(rule.len() < 1000, "Verification rule cannot be over 1000 characters.");
        let mut tokens = tokenize_rule(rule)?;

        let mut input_nos = HashMap::new();
        let mut inputs = Vec::new();
        let mut ctx = CompileContext::new();

        let mut is_operand_context = true;
        'outer: loop {
            if is_operand_context {
                match tokens.pop_front().to_cmd_err(|| "Expected operand.")? {
                    Token::Not => ctx.push_op(CompileOperator::Not),
                    Token::OpenParen => ctx.push_op(CompileOperator::OpenParen),
                    Token::Literal(b) => {
                        ctx.push_term(RuleOp::Literal(b));
                        is_operand_context = false;
                    }
                    Token::Term(start, body) => {
                        if start == "custom_rule" || start == "builtin_rule" {
                            let source = if start == "custom_rule" {
                                RuleSource::CustomRule(body.to_owned())
                            } else {
                                RuleSource::BuiltinRule(body.to_owned())
                            };
                            let no = match input_nos.get(&source) {
                                Some(&res) => res,
                                None => {
                                    let idx = inputs.len();
                                    input_nos.insert(source.clone(), idx);
                                    inputs.push(source);
                                    idx
                                }
                            };
                            ctx.push_term(RuleOp::Read(no))
                        } else {
                            ctx.push_term(parse_term(start, body)?)
                        }
                        is_operand_context = false;
                    }
                    tok => cmd_error!("Expected operand, found '{}'.", tok),
                }
            } else if let Some(token) = tokens.pop_front() {
                match token {
                    Token::CloseParen => {
                        ctx.pop_op_stack(CompileOperator::Any)?;
                        ctx.pop_open_paren()?;
                    }
                    Token::And => {
                        ctx.pop_op_stack(CompileOperator::And)?;
                        ctx.push_op_skip(RuleResult::False, CompileOperator::And);
                        is_operand_context = true;
                    }
                    Token::Or => {
                        ctx.pop_op_stack(CompileOperator::Or)?;
                        ctx.push_op_skip(RuleResult::True, CompileOperator::Or);
                        is_operand_context = true;
                    }
                    tok => cmd_error!("Expected operator, found '{}'.", tok),
                }
            } else {
                ctx.pop_op_stack(CompileOperator::Any)?;
                cmd_ensure!(ctx.op_stack.is_empty(), "Unbalanced parentheses.");
                break
            }
        }

        Ok(VerificationRule { inputs, ops: ctx.ops, op_id_count: ctx.op_id })
    }

    pub fn has_builtin(rule_name: &str) -> bool {
        DEFAULT_RULES.contains_key(&rule_name)
    }
    pub fn get_builtin(rule_name: &str) -> Option<VerificationRule> {
        DEFAULT_RULES.get(&rule_name).cloned()
    }
}
impl fmt::Display for VerificationRule {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "VerificationRule {{")?;
        if !self.inputs.is_empty() {
            writeln!(f, "\t(inputs)")?;
            for input in &self.inputs {
                writeln!(f, "\t\t{:?}", input)?;
            }
            writeln!(f)?;
            writeln!(f, "\t(instructions)")?;
        }
        disasm(&self.ops, if self.inputs.is_empty() { "\t" } else { "\t\t" }, f)?;
        writeln!(f, "}}")?;
        Ok(())
    }
}

struct VerificationCountContext {
    username: bool, dev_trust_level: bool, badges: bool, groups: bool,
    profile_exists: bool, player_badges: HashSet<u64>, owns_asset: HashSet<u64>,
}
impl VerificationCountContext {
    fn new() -> VerificationCountContext {
        VerificationCountContext {
            username: false, dev_trust_level: false, badges: false, groups: false,
            profile_exists: false, player_badges: HashSet::new(), owns_asset: HashSet::new(),
        }
    }

    fn uses_is_banned(&mut self) {
        self.profile_exists = true;
    }
    fn uses_username(&mut self) {
        self.username = true;
    }
    fn uses_dev_trust_level(&mut self) {
        self.uses_username();
        self.dev_trust_level = true;
    }
    fn uses_badges(&mut self) {
        self.badges = true;
    }
    fn uses_groups(&mut self) {
        self.groups = true;
    }
    fn uses_has_player_badge(&mut self, badge_id: u64) {
        self.player_badges.insert(badge_id);
    }
    fn uses_owns_asset(&mut self, asset_id: u64) {
        self.owns_asset.insert(asset_id);
    }

    fn count(&self) -> usize {
        let mut count = 0;
        if self.username        { count += 1 }
        if self.dev_trust_level { count += 1 }
        if self.badges          { count += 1 }
        if self.groups          { count += 1 }
        count += self.player_badges.len();
        count += self.owns_asset.len();
        count
    }
}

struct ValueCache<T>(Option<Option<T>>);
impl <T> ValueCache<T> {
    fn new() -> ValueCache<T> {
        ValueCache(None)
    }

    fn get_cached(
        &mut self, f: impl FnOnce() -> Result<T>, g: impl FnOnce(&T) -> bool
    ) -> RuleResult {
        if self.0.is_none() {
            self.0 = Some(f().ok())
        }
        match self.0.as_ref().unwrap().as_ref() {
            Some(x) => g(x).into(),
            None => RuleResult::Error,
        }
    }
}

struct VerificationContext {
    user_id: RobloxUserID,
    is_banned: ValueCache<bool>, dev_trust_level: ValueCache<Option<u32>>,
    badges: ValueCache<HashSet<String>>, groups: ValueCache<HashMap<u64, u32>>,
    player_badges: HashMap<u64, RuleResult>, owns_asset: HashMap<u64, RuleResult>,
}
impl VerificationContext {
    fn new(user_id: RobloxUserID) -> VerificationContext {
        VerificationContext {
            user_id,
            is_banned: ValueCache::new(), dev_trust_level: ValueCache::new(),
            badges: ValueCache::new(), groups: ValueCache::new(),
            player_badges: HashMap::new(), owns_asset: HashMap::new(),
        }
    }

    fn is_banned(&mut self) -> RuleResult {
        let id = self.user_id;
        self.is_banned.get_cached(|| api::web_profile_exists(id), |x| *x)
    }
    fn has_trust_level(&mut self, condition: Condition) -> RuleResult {
        let id = self.user_id;
        self.dev_trust_level.get_cached(|| api::get_dev_trust_level(&id.lookup_username()?),
                                        |x| match *x {
                                            Some(x) => condition.satisifies(x),
                                            None => false,
                                        })
    }
    fn has_roblox_badge(&mut self, asset_id: &str) -> RuleResult {
        let id = self.user_id;
        self.badges.get_cached(|| api::get_roblox_badges(id), |x| x.contains(asset_id))
    }
    fn is_in_group(&mut self, group_id: u64, rank: Option<Condition>) -> RuleResult {
        let id = self.user_id;
        self.groups.get_cached(|| api::get_player_groups(id),
                               |x| match rank {
                                   Some(r) => x.get(&group_id).map_or(false, |y| r.satisifies(*y)),
                                   None => x.contains_key(&group_id),
                               })
    }
    fn has_player_badge(&mut self, badge_id: u64) -> RuleResult {
        let id = self.user_id;
        *self.player_badges.entry(badge_id).or_insert_with(||
            api::has_player_badge(id, badge_id).ok().into()
        )
    }
    fn owns_asset(&mut self, asset_id: u64) -> RuleResult {
        let id = self.user_id;
        *self.owns_asset.entry(asset_id).or_insert_with(||
            api::owns_asset(id, asset_id).ok().into()
        )
    }
}

struct RuleResolutionContext {
    needed_dependencies: VecDeque<RuleSource>,
    found_rules: HashMap<RuleSource, (VerificationRule, bool)>,
}
impl RuleResolutionContext {
    fn new() -> RuleResolutionContext {
        RuleResolutionContext {
            needed_dependencies: VecDeque::new(), found_rules: HashMap::new(),
        }
    }

    fn add_rule(&mut self, source: RuleSource, rule: VerificationRule, is_output: bool) {
        for dep in &rule.inputs {
            if !self.found_rules.contains_key(&dep) {
                self.needed_dependencies.push_back(dep.clone());
            }
        }
        self.found_rules.insert(source, (rule, is_output));
    }
    fn next_needed(&mut self) -> Option<RuleSource> {
        while let Some(source) = self.needed_dependencies.pop_front() {
            if !self.found_rules.contains_key(&source) {
                return Some(source)
            }
        }
        None
    }

    fn link(mut self) -> Result<VerificationSet> {
        let mut is_refed = HashSet::new();
        for (_, &(ref rule, _)) in &self.found_rules {
            for input in &rule.inputs {
                is_refed.insert(input.clone());
            }
        }

        let mut ops = Vec::new();
        let mut linked = HashMap::new();
        let mut var_count = 0;
        let mut skip_base = 0;
        let mut max_stack = 0;
        loop {
            let mut unresolved_rules = HashMap::new();
            let mut any_resolved = false;
            for (source, (rule, is_output)) in self.found_rules.drain() {
                if rule.inputs.iter().all(|x| linked.contains_key(x)) {
                    let output_as = if is_output {
                        match source {
                            RuleSource::CustomRule(ref rule_name) => Some(rule_name.clone()),
                            RuleSource::BuiltinRule(ref rule_name) => Some(rule_name.clone()),
                        }
                    } else {
                        None
                    };
                    let mut current_stack = 0;
                    for op in rule.ops {
                        let raw_new_current_stack = current_stack as isize + op.stack_change();
                        ensure!(raw_new_current_stack >= 0, "Internal error: Stack underflow.");
                        current_stack = raw_new_current_stack as usize;
                        if current_stack > max_stack {
                            max_stack = current_stack;
                        }

                        match op {
                            RuleOp::Read(i) =>
                                ops.push(RuleOp::Read(linked[&rule.inputs[i]])),
                            RuleOp::StartSkip(skip_cond, id) => {
                                ensure!(id < rule.op_id_count, "Internal error: Invalid skip ID.");
                                ops.push(RuleOp::StartSkip(skip_cond, skip_base + id))
                            }
                            RuleOp::Operator(Some(id), op) => {
                                ensure!(id < rule.op_id_count, "Internal error: Invalid skip ID.");
                                ops.push(RuleOp::Operator(Some(skip_base + id), op))
                            }
                            op => ops.push(op),
                        }
                    }
                    ensure!(current_stack == 1, "Internal error: Rule returns wrong value count!");
                    let var = if is_refed.contains(&source) {
                        let opt = Some(var_count);
                        linked.insert(source, var_count);
                        var_count += 1;
                        opt
                    } else {
                        None
                    };
                    ops.push(RuleOp::Output(var, output_as));
                    skip_base += rule.op_id_count;
                    any_resolved = true;
                } else {
                    unresolved_rules.insert(source, (rule, is_output));
                }
            }
            if unresolved_rules.is_empty() {
                break
            }
            cmd_ensure!(any_resolved, "Circular reference in rules!");
            self.found_rules = unresolved_rules;
        }
        let mut skips = vec![usize::max_value(); skip_base];
        for (i, op) in ops.iter().enumerate() {
            match *op {
                RuleOp::Operator(Some(skip_id), _) => skips[skip_id] = i,
                _ => { }
            }
        }
        Ok(VerificationSet { ops, skips, stack_base: var_count, mem_size: var_count + max_stack })
    }
}

struct State(Vec<RuleResult>, usize);
impl State {
    fn get_var(&mut self, var: usize) -> RuleResult {
        self.0[var]
    }
    fn set_var(&mut self, var: usize, val: RuleResult) {
        self.0[var] = val;
    }
    fn peek(&self) -> RuleResult {
        self.0[self.1 - 1]
    }
    fn pop(&mut self) -> RuleResult {
        self.1 -= 1;
        self.0[self.1]
    }
    fn push(&mut self, value: RuleResult) {
        self.0[self.1] = value;
        self.1 += 1;
    }
}

#[derive(Clone, Debug)]
pub struct VerificationSet {
    ops: Vec<RuleOp>, skips: Vec<usize>, stack_base: usize, mem_size: usize,
}
impl VerificationSet {
    pub fn compile(
        active_rules: &[&str],
        mut lookup_custom_rule: impl FnMut(&str) -> Result<Option<VerificationRule>>,
    ) -> Result<VerificationSet> {
        let mut resolve_ctx = RuleResolutionContext::new();

        for &rule_name in active_rules {
            match lookup_custom_rule(rule_name)? {
                Some(rule) =>
                    resolve_ctx.add_rule(RuleSource::CustomRule(rule_name.to_string()),
                                         rule, true),
                None => match VerificationRule::get_builtin(rule_name) {
                    Some(rule) =>
                        resolve_ctx.add_rule(RuleSource::BuiltinRule(rule_name.to_string()),
                                             rule, true),
                    None => cmd_error!("Unknown rule {}.", rule_name),
                }
            }
        }
        while let Some(rule_source) = resolve_ctx.next_needed() {
            match rule_source.clone() {
                RuleSource::BuiltinRule(rule_name) =>
                    match VerificationRule::get_builtin(&rule_name) {
                        Some(rule) =>
                            resolve_ctx.add_rule(RuleSource::BuiltinRule(rule_name), rule, false),
                        None => cmd_error!("Unknown built-in rule {}.", rule_name),
                    },
                RuleSource::CustomRule(rule_name) =>
                    match lookup_custom_rule(&rule_name)? {
                        Some(rule) =>
                            resolve_ctx.add_rule(RuleSource::CustomRule(rule_name), rule, false),
                        None => cmd_error!("Unknown custom rule {}.", rule_name),
                    },
            }
        }

        resolve_ctx.link()
    }

    pub fn instruction_count(&self) -> usize {
        self.ops.len()
    }
    pub fn max_web_requests(&self) -> usize {
        let mut ctx = VerificationCountContext::new();
        for op in &self.ops {
            match *op {
                RuleOp::CheckBadge(_) => ctx.uses_badges(),
                RuleOp::CheckPlayerBadge(id) => ctx.uses_has_player_badge(id),
                RuleOp::CheckOwnsAsset(asset) => ctx.uses_owns_asset(asset),
                RuleOp::CheckInGroup(_, _) => ctx.uses_groups(),
                RuleOp::CheckDevTrustLevel(_) => ctx.uses_dev_trust_level(),
                RuleOp::CheckIsBanned => ctx.uses_is_banned(),
                _ => { }
            }
        }
        ctx.count()
    }

    pub fn verify(&self, id: RobloxUserID) -> Result<HashMap<&str, RuleResult>> {
        let mut state = State(vec![RuleResult::Error; self.mem_size], self.stack_base);
        let mut ctx = VerificationContext::new(id);
        let mut outputs = HashMap::new();
        let mut ip = 0;
        while ip < self.ops.len() {
            match self.ops[ip] {
                RuleOp::Read(var) => {
                    let val = state.get_var(var);
                    state.push(val)
                },
                RuleOp::Output(ref var, ref name) => {
                    let val = state.pop();
                    if let Some(var) = *var {
                        state.set_var(var, val);
                    }
                    if let Some(ref name) = name {
                        outputs.insert(name.as_str(), val);
                    }
                },
                RuleOp::Literal(b) => state.push(b),
                RuleOp::StartSkip(skip_cond, skip_id) => {
                    let current = state.peek();
                    if current == RuleResult::Error || current == skip_cond {
                        ip = self.skips[skip_id];
                    }
                },
                RuleOp::Operator(_, op) => {
                    let val = match op {
                        Operator::Not => match state.pop() {
                            RuleResult::True => RuleResult::False,
                            RuleResult::False => RuleResult::True,
                            RuleResult::Error => RuleResult::Error,
                        },
                        Operator::And => match (state.pop(), state.pop()) {
                            (RuleResult::True, x) => x,
                            (x, _) => x,
                        },
                        Operator::Or  => match (state.pop(), state.pop()) {
                            (RuleResult::False, x) => x,
                            (x, _) => x,
                        },
                    };
                    state.push(val)
                }
                RuleOp::CheckBadge(ref name) => state.push(ctx.has_roblox_badge(name)),
                RuleOp::CheckPlayerBadge(id) => state.push(ctx.has_player_badge(id)),
                RuleOp::CheckOwnsAsset(asset) => state.push(ctx.owns_asset(asset)),
                RuleOp::CheckInGroup(group, rank) => state.push(ctx.is_in_group(group, rank)),
                RuleOp::CheckDevTrustLevel(check) => state.push(ctx.has_trust_level(check)),
                RuleOp::CheckIsBanned => state.push(ctx.is_banned()),
            }
            ip += 1;
        }
        Ok(outputs)
    }
}
impl fmt::Display for VerificationSet {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "VerificationSet {{")?;
        disasm(&self.ops, "\t", f)?;
        writeln!(f, "}}")?;
        Ok(())
    }
}