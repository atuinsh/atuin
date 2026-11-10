//! The Luau VM a config file runs in.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt as _, StdLib, Table, Value, VmState};
use serde::Deserialize;

use super::header::Version;
use super::{Config, ConfigFile, LoadError, Script};

/// The globals a config file sees, beside `atuin`
const GLOBALS: [&str; 19] = [
    "assert", "error", "ipairs", "next", "pairs", "pcall", "rawequal", "rawget", "rawlen",
    "select", "tonumber", "tostring", "type", "typeof", "unpack", "xpcall", "math", "string",
    "table",
];

/// The most a file's VM may allocate: a config is a few small tables.
const MEMORY_LIMIT_BYTES: usize = 8 * 1024 * 1024;

/// The longest a file may run: one runs in microseconds, so this only stops one that never ends.
const TIME_LIMIT: Duration = Duration::from_millis(500);

/// The `atuin` global, as a file leaves it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AtuinGlobal {
    config: Config,
}

/// A fresh VM with one script compiled in it. [`Vm::eval`] consumes the VM, so no two files share
/// one.
#[derive(Debug)]
pub struct Vm {
    path: PathBuf,
    lua: Lua,
    env: Table,
    chunk: Function,
}

impl Vm {
    /// Builds a VM for the script's version, and compiles the script in it.
    pub fn load(script: Script) -> Result<Self, LoadError> {
        let Script {
            path,
            bytes,
            header,
        } = script;
        let run = |source| LoadError::Run {
            path: path.clone(),
            source,
        };

        let libraries = StdLib::STRING | StdLib::TABLE | StdLib::MATH;
        let lua = Lua::new_with(libraries, LuaOptions::new()).map_err(run)?;
        let env = match header.version {
            Version::V1 => Self::v1_environment(&lua),
        }
        .map_err(run)?;

        lua.sandbox(true).map_err(run)?;
        lua.set_memory_limit(MEMORY_LIMIT_BYTES).map_err(run)?;
        let chunk = lua
            .load(bytes)
            .set_name(format!("@{}", path.display()))
            .set_mode(ChunkMode::Text)
            .set_environment(env.clone())
            .into_function()
            .map_err(run)?;

        Ok(Self {
            path,
            lua,
            env,
            chunk,
        })
    }

    /// Runs the script and reads the config it sets as `atuin.config`.
    pub fn eval(self) -> Result<ConfigFile, LoadError> {
        let deadline = Instant::now() + TIME_LIMIT;
        self.lua.set_interrupt(move |_| {
            if Instant::now() < deadline {
                Ok(VmState::Continue)
            } else {
                Err(mlua::Error::runtime(format!("ran past {TIME_LIMIT:?}")))
            }
        });

        if let Err(source) = self.chunk.call::<()>(()) {
            return Err(LoadError::Run {
                path: self.path,
                source,
            });
        }

        let atuin = self
            .env
            .raw_get("atuin")
            .and_then(|atuin: Value| self.lua.from_value::<AtuinGlobal>(atuin));
        match atuin {
            Ok(AtuinGlobal { config }) => Ok(ConfigFile {
                path: self.path,
                config,
            }),
            Err(source) => Err(LoadError::Invalid {
                path: self.path,
                source,
            }),
        }
    }

    fn v1_environment(lua: &Lua) -> mlua::Result<Table> {
        let globals = lua.globals();
        let env = lua.create_table()?;
        for name in GLOBALS {
            env.raw_set(name, globals.raw_get::<Value>(name)?)?;
        }

        env.raw_set("atuin", lua.create_table()?)?;
        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::lua_config::Config;
    use crate::lua_config::header::Header;

    fn script(bytes: impl Into<Vec<u8>>) -> Script {
        Script {
            path: PathBuf::from("atuin.lua"),
            bytes: bytes.into(),
            header: Header {
                version: Version::V1,
            },
        }
    }

    fn run(source: &str) -> Result<Config, LoadError> {
        Ok(Vm::load(script(source))?.eval()?.config)
    }

    /// A project's file comes with a repository someone else wrote, so it gets nothing that reaches
    /// past its own values: mlua's `require` reads the disk, `print` writes to the daemon's output.
    #[rstest]
    fn a_file_reaches_nothing_but_its_values(
        #[values(
            "require",
            "print",
            "loadstring",
            "getfenv",
            "setfenv",
            "getmetatable",
            "setmetatable",
            "collectgarbage",
            "gcinfo",
            "newproxy",
            "_G",
            "io",
            "os",
            "debug",
            "coroutine",
            "buffer",
            "utf8",
            "bit32",
            "vector"
        )]
        global: &str,
    ) {
        let config = run(&format!("atuin.config = {{ orgs = {{ type({global}) }} }}"));

        let orgs = config.expect("a config").orgs.expect("orgs");
        let types: Vec<String> = orgs.iter().map(ToString::to_string).collect();
        assert_eq!(types, ["nil"], "{global} is reachable");
    }

    #[rstest]
    #[case::syntax(r"atuin.config = {")]
    #[case::raises(r#"error("no")"#)]
    fn a_file_that_fails_to_run_says_so(#[case] source: &str) {
        assert!(matches!(run(source), Err(LoadError::Run { .. })), "{source}");
    }

    /// `pcall` catches the stop where it's raised, but the next check outside it raises again.
    #[rstest]
    #[case::looping(r"while true do end")]
    #[case::catching_the_stop(r"while true do pcall(function() while true do end end) end")]
    #[case::recursing(r"local function f() pcall(f) f() end f()")]
    fn a_file_that_runs_forever_is_stopped(#[case] source: &str) {
        let err = run(source).expect_err("a stopped file");

        assert!(
            matches!(&err, LoadError::Run { source: mlua::Error::RuntimeError(message), .. } if message.contains("ran past")),
            "{err:?}"
        );
    }

    #[rstest]
    fn a_file_that_takes_too_much_memory_is_stopped() {
        let err = run(r#"local s = string.rep("x", 16 * 1024 * 1024) atuin.config = {}"#)
            .expect_err("a stopped file");

        assert!(
            matches!(err, LoadError::Run {
                source: mlua::Error::MemoryError(_),
                ..
            }),
            "{err:?}"
        );
    }

    /// Luau doesn't verify bytecode, which can crash the VM, so a file is only ever compiled.
    #[rstest]
    fn bytecode_is_never_loaded() {
        let bytecode = mlua::chunk::Compiler::new().compile("atuin.config = {}").expect("bytecode");

        let loaded = Vm::load(script(bytecode));

        assert!(matches!(loaded, Err(LoadError::Run { .. })));
    }

    #[rstest]
    fn a_file_may_set_globals_but_not_change_a_library() {
        assert!(run(r"x = 1 atuin.config = {}").is_ok());
        assert!(run(r"string.upper = nil atuin.config = {}").is_err(), "a library was changed");
    }
}
