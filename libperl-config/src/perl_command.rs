use super::process_util::*;

use regex::Regex;

pub struct PerlCommand {
    perl: String,    
}

impl Default for PerlCommand {
    /// Uses the perl named by the `PERL` environment variable when set
    /// (and non-empty), falling back to `perl` on `PATH`. Build tools
    /// like ExtUtils::MakeMaker postambles pass `PERL=$(FULLPERL)` so
    /// that the perl running Makefile.PL and the perl being linked
    /// against are the same.
    fn default() -> Self {
        // Instruct cargo to re-run the calling build script when the
        // selected perl changes. Safe here: this crate is only used
        // from build scripts.
        println!("cargo:rerun-if-env-changed=PERL");
        Self {
            perl: std::env::var("PERL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| String::from("perl")),
        }
    }
}

impl PerlCommand {

    pub fn new(perl: &str) -> Self {
        Self {
            perl: String::from(perl),
        }
    }

    pub fn command(&self, args: &[&str]) -> Command {
        make_command(self.perl.as_str(), args)
    }

    pub fn read_raw_config(&self, configs: &[&str]) -> Result<String, Error> {
        let script = ["-wle", r#"
    use strict;
    use Config;
    print join "\t", $_, ($Config{$_} // '')
      for @ARGV ? @ARGV : sort keys %Config;
    "#
        ];
        let mut cmd = self.command(&[&script[..], configs].concat());
        
        process_command_output(cmd.output()?)
    }

    pub fn read_ccopts(&self) -> Result<Vec<String>, Error> {
        self.read_embed_opts("ccopts", r"^-[ID]")
    }

    pub fn read_ldopts(&self) -> Result<Vec<String>, Error> {
        // Win32 perl passes libperl as a bare file path ("...\CORE\libperl540.a")
        // instead of -lperl540, so library files are kept too.
        self.read_embed_opts("ldopts",  r"^-[lL]|\.(a|lib)$")
    }

    pub fn read_raw_embed_opts(&self, cmd: &str) -> Result<String, Error> {
        let mut cmd = self.command(
            &["-MExtUtils::Embed", "-e", cmd],
        );

        process_command_output(cmd.output()?)
    }

    pub fn read_embed_opts(&self, cmd: &str, prefix: &str) -> Result<Vec<String>, Error> {
        let out_str = self.read_raw_embed_opts(cmd)?;

        let re = Regex::new(prefix).unwrap();
        Ok(split_embed_opts(&out_str)
            .into_iter()
           .filter(|s| re.is_match(s))
           .collect())
    }

    pub fn emit_cargo_ldopts(&self) {
        let ldopts = self.read_ldopts().unwrap();
        println!("# perl ldopts = {:?}, ", ldopts);

        for opt in self.read_ldopts().unwrap().iter() {
            if opt.starts_with("-L") {
                let libpath = opt.get(2..).unwrap();
                println!("cargo:rustc-link-search={}", libpath);
                // rpath is ELF-only.
                // Windows finds perl5xx.dll via PATH.
                let is_windows_target = std::env::var_os("CARGO_CFG_TARGET_OS").map_or(cfg!(windows), |env_var| env_var == "windows");

                if !is_windows_target
                    && std::path::Path::new(libpath).file_name()
                    == Some(std::ffi::OsStr::new("CORE")) {
                        // Embed rpath into ALL link products of the calling
                        // crate — `cargo:rustc-link-arg=` covers cdylibs,
                        // bins, examples, AND `cargo test` binaries. The
                        // earlier `cargo:rustc-cdylib-link-arg=` form only
                        // covered cdylibs, so test binaries on perls
                        // installed in non-default locations (e.g. via
                        // `shogo82148/actions-setup-perl@v1`) failed at
                        // runtime with `libperl.so: cannot open shared
                        // object file`.
                        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", libpath);
                    }
            }
            else if opt.starts_with("-l") {
                println!("cargo:rustc-link-lib={}", opt.get(2..).unwrap());
            } else if let Some(name) = lib_name_from_path(opt) {
                // Bare library file (Win32):
                // link searchdir + library name
                if let Some(dir) = std::path::Path::new(opt).parent() {
                    println!("cargo:rustc-link-search=native={}",
 dir.display());
                }
                println!("cargo:rustc-link-lib={}", name)
            }
        }
    }
}

/*
 * ############################################
 * # HELPER FUNCTIONS NEEDED FOR WIN32 COMPAT #
 * ############################################
 */
/// Splits ExtUtils::Embed output into arguments.
/// Win32 perl quotes paths (`-I"C:\...\CORE"`, `"C:\...\libperl540.a"`),
/// so double quotes group whitespace and are removed.
/// Backslashes are Windows path separators and are kept as-is (no escape handling).
fn split_embed_opts(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_arg = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_arg = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_arg {
                    args.push(std::mem::take(&mut cur));
                    has_arg = false;
                }
            }
            c => {
                cur.push(c);
                has_arg = true;
            }
        }
    }
    if has_arg {
        args.push(cur);
    }
    args
}

/// "C:\...\libperl540.a" -> "perl540", "C:\...\perl540.lib" -> "perl540".
fn lib_name_from_path(opt:&str) -> Option<String> {
    use std::path::Path;

    let path = Path::new(opt);
    let stem = path.file_stem()?.to_str()?;
    match path.extension()?.to_str()? {
        //libperlXYZ.a => perlXYZ
        //   perlXYZ.a => perlXYZ
        "a" => Some(stem.strip_prefix("lib").unwrap_or(stem).to_string()),
        "lib" => Some(stem.to_string()),
        _ => None
    }
}