//! Reads a compiled chunk and says what it does that a replay can't survive, so the rules hold on the
//! bytes that run and not on what a compiler promised. Written against Luau 0.740's `lvmload.cpp`,
//! bytecode versions 9 to 14: anything it can't read is refused, so a Luau upgrade that moves the format
//! fails here rather than letting a chunk through unread.

/// Luau's opcodes this reads, from `Bytecode.h`'s `LuauOpcode`.
mod op {
    pub const GETGLOBAL: u8 = 7;
    pub const SETGLOBAL: u8 = 8;
    pub const LOADN: u8 = 4;
    pub const LOADK: u8 = 5;
    pub const SETUPVAL: u8 = 10;
    pub const GETIMPORT: u8 = 12;
    pub const GETTABLEKS: u8 = 15;
    pub const SETTABLEKS: u8 = 16;
    pub const NAMECALL: u8 = 20;
    pub const MOD: u8 = 37;
    pub const POW: u8 = 38;
    pub const POWK: u8 = 44;
    pub const NEWTABLE: u8 = 53;
    pub const SETLIST: u8 = 55;
    pub const FORGLOOP: u8 = 58;
    pub const FASTCALL3: u8 = 60;
    pub const LOADKX: u8 = 66;
    pub const FASTCALL: u8 = 68;
    pub const FASTCALL1: u8 = 73;
    pub const FASTCALL2: u8 = 74;
    pub const FASTCALL2K: u8 = 75;
    pub const FORGPREP: u8 = 76;
    pub const NEWCLASS: u8 = 90;
    pub const LAST: u8 = 90;
}

/// Whether an instruction has a second word: `getOpLength` in `BytecodeUtils.h`.
const fn has_aux(opcode: u8) -> bool {
    matches!(
        opcode,
        op::GETGLOBAL
            | op::SETGLOBAL
            | op::GETIMPORT
            | op::GETTABLEKS
            | op::SETTABLEKS
            | op::NAMECALL
            | 27..=32 // the JUMPIF comparisons
            | op::NEWTABLE
            | op::SETLIST
            | op::FORGLOOP
            | op::LOADKX
            | op::FASTCALL2
            | op::FASTCALL2K
            | op::FASTCALL3
            | 77..=80 // JUMPXEQK*
            | 83..=88 // user data, classes, call feedback
            | op::NEWCLASS
    )
}

/// Library calls a chunk may make with no global lookup, by `LuauBuiltinFunction` number. Everything else
/// in that list goes to the C library or hands a script something the allowlist took away, and FASTCALL
/// reaches it even when the global is gone.
const fn builtin_allowed(id: u8) -> bool {
    matches!(
        id,
        1 // assert
            | 2 // math.abs
            | 7 // math.ceil
            | 10 // math.deg
            | 12 // math.floor
            | 18 | 19 // math.max, math.min
            | 22 // math.rad
            | 25 // math.sqrt
            | 28..=39 // bit32
            | 40 // type
            | 41..=43 // string.byte, char, len
            | 44 // typeof
            | 45 // string.sub
            | 46..=48 // math.clamp, sign, round
            | 49..=51 // rawset, rawget, rawequal
            | 52 | 53 // table.insert, unpack
            | 54 // vector.create
            | 55 | 56 // bit32.countlz, countrz
            | 57 // select
            | 58 // rawlen
            | 59 // bit32.extract
            | 64 // bit32.byteswap
            | 78..=90 // vector's operations and math.lerp: arithmetic and sqrt
            | 91..=93 // math.isnan, isinf, isfinite
    )
}

/// What's in a chunk that the rules speak to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Findings {
    /// `POW` by a run-time exponent, which calls C's `pow`.
    pub pow: u32,
    /// `POWK` by a constant other than 2, 3 and 0.5, the three the VM works out exactly.
    pub pow_constants: Vec<f64>,
    /// `POW` and `MOD` of two literals: what a compiler above level 0 works out on the host, in C.
    /// Seen only in level 0's bytes, where nothing's been folded.
    pub folded: u32,
    /// `POW` by a literal, at level 0, where a literal exponent is a `POW` too: what it'd be above it.
    pub pow_literals: Vec<f64>,
    /// Library calls by `FASTCALL` outside the allowed ones, as `LuauBuiltinFunction` numbers.
    pub fastcalls: Vec<u8>,
    /// Library calls by `FASTCALL` of any kind, the allowed ones too.
    pub fastcalls_all: Vec<u8>,
    /// `FORGPREP`: a loop over a bare table's hash part, or through an iterator picked at run time.
    pub generic_for: u32,
    /// `SETGLOBAL`: the environment is read-only, so this can only fail.
    pub global_writes: u32,
    /// `SETUPVAL`: a closure writing a local it shares, so state across hooks if that's a chunk's local.
    pub upvalue_writes: u32,
    /// Instructions read, across every function.
    pub instructions: u32,
    /// Every number in a constant table, by its bits: what a compiler left of what it folded.
    pub numbers: Vec<u64>,
}

impl Findings {
    /// The first rule this chunk breaks, read from its bytes as they ship.
    #[must_use]
    pub fn violation(&self) -> Option<String> {
        if self.pow > 0 {
            return Some("`^` by anything but the literals 2, 3 and 0.5 reaches the C library".to_owned());
        }
        if let Some(constant) = self.pow_constants.first() {
            return Some(format!("`^ {constant}` reaches the C library: only 2, 3 and 0.5 are exact"));
        }
        if let Some(id) = self.fastcalls.first() {
            return Some(format!("builtin {id} isn't on the allowlist"));
        }
        if self.generic_for > 0 {
            return Some("`for .. in` needs `ipairs` or `pairs`: a bare table walks its hash".to_owned());
        }
        if self.global_writes > 0 {
            return Some("a global write: the environment is read-only".to_owned());
        }
        if self.upvalue_writes > 0 {
            return Some(
                "a write to an outer local: nothing may last between hooks but the board".to_owned(),
            );
        }
        None
    }

    /// The first rule this chunk breaks, read from its bytes at level 0, before folding hides it: there
    /// a `^` is always a `POW` with its exponent in a register, so what loaded that register says.
    #[must_use]
    pub fn violation_unfolded(&self) -> Option<String> {
        if self.folded > 0 {
            return Some(
                "`^` or `%` of two literals is worked out by the host's C library at compile".to_owned(),
            );
        }
        if let Some(constant) = self.pow_literals.iter().find(|k| !exact_power(**k)) {
            return Some(format!("`^ {constant}` reaches the C library: only 2, 3 and 0.5 are exact"));
        }
        if self.pow > 0 {
            return Some("`^` by anything but the literals 2, 3 and 0.5 reaches the C library".to_owned());
        }
        None
    }
}

/// The exponents the VM works out with no C library.
fn exact_power(exponent: f64) -> bool {
    exponent == 2.0 || exponent == 3.0 || exponent == 0.5
}

/// What an instruction loads into `register`, if it loads a literal number there.
fn literal(word: u32, register: u8, constants: &[Option<f64>]) -> Option<f64> {
    if ((word >> 8) & 0xff) as u8 != register {
        return None;
    }
    let d = word.cast_signed() >> 16;
    match (word & 0xff) as u8 {
        op::LOADN => Some(f64::from(d)),
        op::LOADK => constants.get(d.cast_unsigned() as usize).copied().flatten(),
        _ => None,
    }
}

/// Bytes that aren't a chunk this reader knows.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    #[error("the chunk ends early")]
    Short,
    #[error("bytecode version {0} isn't one this reads")]
    Version(u8),
    #[error("types version {0} isn't one this reads")]
    TypesVersion(u8),
    #[error("opcode {0} isn't one this reads")]
    Opcode(u8),
    #[error("constant tag {0} isn't one this reads")]
    Constant(u8),
    #[error("a compile error: {0}")]
    Compile(String),
    #[error("{0} bytes are left over")]
    Trailing(usize),
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, ScanError> {
        let byte = *self.bytes.get(self.at).ok_or(ScanError::Short)?;
        self.at += 1;
        Ok(byte)
    }

    fn take(&mut self, count: usize) -> Result<&[u8], ScanError> {
        let end = self.at.checked_add(count).ok_or(ScanError::Short)?;
        let bytes = self.bytes.get(self.at..end).ok_or(ScanError::Short)?;
        self.at = end;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32, ScanError> {
        let word = self.take(4)?;
        Ok(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
    }

    fn u64(&mut self) -> Result<u64, ScanError> {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(bytes))
    }

    fn varint(&mut self) -> Result<u64, ScanError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(ScanError::Short)
    }

    fn count(&mut self) -> Result<usize, ScanError> {
        usize::try_from(self.varint()?).map_err(|_| ScanError::Short)
    }
}

/// Reads every function of a compiled chunk.
pub fn scan(bytes: &[u8]) -> Result<Findings, ScanError> {
    let mut reader = Reader { bytes, at: 0 };
    let version = reader.u8()?;
    if version == 0 {
        return Err(ScanError::Compile(String::from_utf8_lossy(&bytes[1..]).into_owned()));
    }
    if !(9..=14).contains(&version) {
        return Err(ScanError::Version(version));
    }
    let types_version = reader.u8()?;
    if !(1..=3).contains(&types_version) {
        return Err(ScanError::TypesVersion(types_version));
    }
    for _ in 0..reader.count()? {
        let length = reader.count()?;
        reader.take(length)?;
    }
    if types_version == 3 {
        while reader.u8()? != 0 {
            reader.varint()?;
        }
    }
    let mut findings = Findings::default();
    for _ in 0..reader.count()? {
        if version >= 12 {
            reader.varint()?;
        }
        function(&mut reader, version, &mut findings)?;
    }
    reader.varint()?; // the main function
    match bytes.len() - reader.at {
        0 => Ok(findings),
        left => Err(ScanError::Trailing(left)),
    }
}

fn function(reader: &mut Reader<'_>, version: u8, findings: &mut Findings) -> Result<(), ScanError> {
    reader.take(4)?; // max stack, parameters, upvalues, varargs
    let flags = reader.u8()?;
    let type_bytes = reader.count()?;
    reader.take(type_bytes)?;

    let code_len = reader.count()?;
    let mut code = Vec::with_capacity(code_len);
    for _ in 0..code_len {
        code.push(reader.u32()?);
    }
    let constants = constants(reader, findings)?;

    let mut pc = 0;
    while pc < code.len() {
        let word = code[pc];
        let opcode = (word & 0xff) as u8;
        if opcode > op::LAST {
            return Err(ScanError::Opcode(opcode));
        }
        let a = ((word >> 8) & 0xff) as u8;
        let c = (word >> 24) as usize;
        findings.instructions += 1;
        match opcode {
            op::POW | op::MOD => {
                let b = ((word >> 16) & 0xff) as u8;
                let exponent = pc.checked_sub(1).and_then(|at| literal(code[at], c as u8, &constants));
                let base = pc.checked_sub(2).and_then(|at| literal(code[at], b, &constants));
                if exponent.is_some() && base.is_some() {
                    findings.folded += 1;
                }
                if opcode == op::POW {
                    match exponent {
                        Some(exponent) => findings.pow_literals.push(exponent),
                        None => findings.pow += 1,
                    }
                }
            }
            op::POWK => match constants.get(c) {
                Some(Some(k)) if exact_power(*k) => {}
                Some(Some(k)) => findings.pow_constants.push(*k),
                _ => findings.pow_constants.push(f64::NAN),
            },
            op::FASTCALL | op::FASTCALL1 | op::FASTCALL2 | op::FASTCALL2K | op::FASTCALL3 => {
                findings.fastcalls_all.push(a);
                if !builtin_allowed(a) {
                    findings.fastcalls.push(a);
                }
            }
            op::FORGPREP => findings.generic_for += 1,
            op::SETGLOBAL => findings.global_writes += 1,
            op::SETUPVAL => findings.upvalue_writes += 1,
            _ => {}
        }
        pc += if has_aux(opcode) { 2 } else { 1 };
    }

    for _ in 0..reader.count()? {
        reader.varint()?; // its child functions
    }
    reader.varint()?; // the line it starts on
    reader.varint()?; // its name
    if reader.u8()? != 0 {
        let gap_log2 = reader.u8()?;
        reader.take(code_len)?;
        let intervals = (code_len.saturating_sub(1) >> gap_log2) + 1;
        reader.take(intervals * 4)?;
    }
    if reader.u8()? != 0 {
        for _ in 0..reader.count()? {
            reader.varint()?;
            reader.varint()?;
            reader.varint()?;
            reader.u8()?;
        }
        for _ in 0..reader.count()? {
            reader.varint()?;
        }
    }
    if version >= 11 {
        for _ in 0..reader.count()? {
            reader.u8()?;
            reader.varint()?;
        }
    }
    if version >= 12 && flags & INLINABLE != 0 {
        reader.varint()?;
    }
    Ok(())
}

/// `LPF_INLINABLE`, which says a function carries a cost.
const INLINABLE: u8 = 1 << 3;

/// A function's constants: the numbers by position, with `None` for anything else.
fn constants(reader: &mut Reader<'_>, findings: &mut Findings) -> Result<Vec<Option<f64>>, ScanError> {
    let count = reader.count()?;
    let mut numbers = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = reader.u8()?;
        let number = match tag {
            0 => None,
            1 => {
                reader.u8()?;
                None
            }
            2 => {
                let bits = reader.u64()?;
                findings.numbers.push(bits);
                Some(f64::from_bits(bits))
            }
            3 | 6 => {
                reader.varint()?;
                None
            }
            4 => {
                reader.u32()?;
                None
            }
            5 => {
                for _ in 0..reader.count()? {
                    reader.varint()?;
                }
                None
            }
            7 => {
                reader.take(16)?;
                None
            }
            8 => {
                for _ in 0..reader.count()? {
                    reader.varint()?;
                    reader.u32()?;
                }
                None
            }
            9 => {
                reader.u8()?;
                reader.varint()?;
                None
            }
            11 => {
                reader.take(32)?;
                None
            }
            tag => return Err(ScanError::Constant(tag)),
        };
        numbers.push(number);
    }
    Ok(numbers)
}
