//! Name and extension mappings adapted from eza v0.18.21 (MIT).
//! See LICENSE-eza.txt and https://github.com/eza-community/eza/tree/v0.18.21/src.

pub(super) fn directory_icon(name: &str) -> Option<char> {
    Some(match name {
        ".config" | "config" | "cron.d" | "cron.daily" | "cron.hourly" | "cron.minutely"
        | "cron.monthly" | "cron.weekly" | "etc" | "include" | "xbps.d" | "xorg.conf.d" => {
            '\u{e5fc}'
        }
        ".git" => '\u{e5fb}',
        ".github" => '\u{e5fd}',
        ".npm" | "node_modules" | "npm_cache" => '\u{e5fa}',
        ".ssh" | "pam.d" | "ssh" | "sudoers.d" => '\u{f08ac}',
        ".Trash" => '\u{f1f8}',
        "Contacts" => '\u{f024c}',
        "Desktop" => '\u{f108}',
        "Downloads" => '\u{f024d}',
        "Favorites" => '\u{f069d}',
        "hidden" => '\u{f179e}',
        "home" => '\u{f10b5}',
        "Mail" => '\u{f01f0}',
        "Movies" => '\u{f0fce}',
        "Music" => '\u{f1359}',
        "Pictures" => '\u{f024f}',
        "Videos" => '\u{f03d}',
        "hi" => '\u{eae8}',
        "cabal" => '\u{e777}',
        "Documents" => '\u{f0c82}',
        _ => return None,
    })
}

pub(super) fn name_icon(name: &str) -> Option<char> {
    Some(match name {
        ".atom" => '\u{e764}',
        ".bashrc" | ".bash_history" | ".bash_logout" | ".bash_profile" | ".cshrc" | ".kshrc"
        | ".login" | ".logout" | ".profile" | ".tcshrc" | ".zlogin" | ".zlogout" | ".zprofile"
        | ".zshenv" | ".zshrc" | ".zsh_history" | ".zsh_sessions" | "bashrc" | "csh.cshrc"
        | "csh.login" | "csh.logout" | "profile" | "zlogin" | "zlogout" | "zprofile" | "zshenv"
        | "zshrc" => '\u{f1183}',
        ".CFUserTextEncoding" | ".DS_Store" | "._DS_Store" | "localized" => '\u{f179}',
        ".clang-format" | ".fennelrc" | ".htaccess" | ".htpasswd" | ".inputrc" | ".Xauthority"
        | ".xinitrc" | ".Xresources" | "bspwmrc" | "config" | "config.status" | "configure.ac"
        | "configure.in" | "crontab" | "crypttab" | "environment" | "hostname" | "inputrc"
        | "shells" => '\u{e615}',
        ".emacs" => '\u{e632}',
        ".eslintrc.cjs" | ".eslintrc.js" | ".eslintrc.json" | ".eslintrc.yaml"
        | ".eslintrc.yml" => '\u{e655}',
        ".gitattributes" | ".gitconfig" | ".gitignore" | ".gitignore_global" | ".gitmodules"
        | ".mailmap" => '\u{f1d3}',
        ".gitlab-ci.yml" => '\u{f296}',
        ".idea" => '\u{e7b5}',
        ".ideavimrc" | ".viminfo" | ".vimrc" => '\u{e7c5}',
        ".node_repl_history" => '\u{e718}',
        ".npmignore"
        | ".npmrc"
        | "npm-shrinkwrap.json"
        | "npmrc"
        | "package-lock.json"
        | "package.json" => '\u{e71e}',
        ".python_history" | "constraints.txt" | "MANIFEST" | "MANIFEST.in" | "pyproject.toml"
        | "requirements.txt" => '\u{e606}',
        ".rustfmt.toml" | "Cargo.lock" | "Cargo.toml" | "release.toml" => '\u{e68b}',
        ".rvm" | ".rvmrc" | "Rakefile" | "rvmrc" => '\u{e21e}',
        ".yarnrc" | "yarn.lock" => '\u{e6a7}',
        "a.out" => '\u{f489}',
        "authorized_keys" | "known_hosts" => '\u{f08c0}',
        "build.gradle.kts"
        | "gradle"
        | "gradle.properties"
        | "gradlew"
        | "gradlew.bat"
        | "settings.gradle.kts" => '\u{e660}',
        "CMakeLists.txt" | "GNUmakefile" | "Makefile" | "makefile" | "Makefile.ac"
        | "Makefile.am" | "Makefile.in" => '\u{e673}',
        "composer.json" | "composer.lock" | "php.ini" => '\u{e73d}',
        "configure" | "dune-project" | "Justfile" => '\u{f0ad}',
        "COPYING" | "COPYRIGHT" | "LICENCE" | "LICENCE.md" | "LICENCE.txt" | "LICENSE"
        | "LICENSE-APACHE" | "LICENSE-MIT" | "LICENSE.md" | "LICENSE.txt" => '\u{f02d}',
        "docker-compose.yml"
        | "Dockerfile"
        | "compose.yaml"
        | "compose.yml"
        | "docker-compose.yaml" => '\u{e650}',
        "dune" => '\u{e67a}',
        "Earthfile" => '\u{f0ac}',
        "go.mod" | "go.sum" | "go.work" => '\u{e65e}',
        "group" | "gshadow" | "passwd" | "shadow" | "sudoers" => '\u{f023}',
        "gruntfile.coffee" | "gruntfile.js" | "gruntfile.ls" => '\u{e611}',
        "gulpfile.coffee" | "gulpfile.js" | "gulpfile.ls" => '\u{e610}',
        "heroku.yml" | "Procfile" => '\u{e77b}',
        "id_dsa" | "id_ecdsa" | "id_ecdsa_sk" | "id_ed25519" | "id_ed25519_sk" | "id_rsa" => {
            '\u{f0306}'
        }
        "Jenkinsfile" => '\u{e66e}',
        "jsconfig.json" => '\u{e74e}',
        "localtime" | "timezone" => '\u{f43a}',
        "PKGBUILD" => '\u{f303}',
        "platformio.ini" => '\u{e682}',
        "pom.xml" => '\u{e674}',
        "README" => '\u{f00ba}',
        "robots.txt" => '\u{f06a9}',
        "rubydoc" => '\u{e73b}',
        "tsconfig.json" => '\u{e628}',
        "Vagrantfile" => '\u{2371}',
        "webpack.config.js" => '\u{f072b}',
        _ => return None,
    })
}

pub(super) fn extension_icon(name: &str) -> Option<char> {
    Some(match name {
        "3mf" | "fbx" | "obj" | "ply" | "stl" => '\u{f01a7}',
        "7z" | "ar" | "arj" | "br" | "bz" | "bz2" | "bz3" | "cpio" | "gz" | "lz" | "lz4"
        | "lzh" | "lzma" | "lzo" | "par" | "rar" | "tar" | "taz" | "tbz" | "tbz2" | "tgz"
        | "tlz" | "txz" | "tz" | "tzo" | "xz" | "z" | "zip" | "zst" => '\u{f410}',
        "a" | "ko" | "so" => '\u{f17c}',
        "acc" | "aif" | "aifc" | "aiff" | "alac" | "ape" | "flac" | "m4a" | "mka" | "mp2"
        | "mp3" | "ogg" | "opus" | "pcm" | "wav" | "wma" | "wv" => '\u{f001}',
        "acf" => '\u{f1b6}',
        "age" | "asc" | "gpg" => '\u{f099d}',
        "ai" => '\u{e7b4}',
        "android" | "apk" => '\u{e70e}',
        "apple" | "bundle" | "dylib" | "localized" | "plist" => '\u{f179}',
        "arw" | "avif" | "bmp" | "cbr" | "cbz" | "cr2" | "dvi" | "gif" | "heic" | "heif"
        | "ico" | "j2c" | "j2k" | "jfi" | "jfif" | "jif" | "jp2" | "jpe" | "jpeg" | "jpf"
        | "jpg" | "jpx" | "jxl" | "nef" | "orf" | "pbm" | "pgm" | "png" | "pnm" | "ppm" | "pxm"
        | "raw" | "tif" | "tiff" | "webp" | "xcf" | "xpm" => '\u{f1c5}',
        "asm" | "s" => '\u{e637}',
        "asp" => '\u{f121}',
        "avi" | "flv" | "h264" | "heics" | "m2ts" | "m2v" | "m4v" | "mkv" | "mov" | "mp4"
        | "mpeg" | "mpg" | "ogm" | "ogv" | "video" | "vob" | "webm" | "wmv" => '\u{f03d}',
        "avro" | "json" | "properties" | "webmanifest" => '\u{e60b}',
        "awk" | "bash" | "bats" | "csh" | "fish" | "ksh" | "nu" | "sh" | "shell" | "zsh" => {
            '\u{f489}'
        }
        "bat" | "com" | "exe" => '\u{ebc4}',
        "bdf" | "eot" | "fnt" | "fon" | "font" | "otf" | "psf" | "ttc" | "ttf" | "woff"
        | "woff2" => '\u{f031}',
        "bib" | "bst" | "cls" | "latex" | "ltx" | "sty" | "tex" => '\u{e69b}',
        "bin" | "elf" | "o" => '\u{eae8}',
        "blend" => '\u{f00ab}',
        "c" | "h" | "inl" | "m" => '\u{e61e}',
        "c++" | "cc" | "cp" | "cpp" | "cxx" | "h++" | "hh" | "hpp" | "hxx" | "mm" => '\u{e61d}',
        "cab" | "cmd" | "msi" | "windows" => '\u{f17a}',
        "cert" | "crt" => '\u{eafa}',
        "cfg" | "conf" | "config" | "editorconfig" | "ini" | "tml" | "toml" => '\u{e615}',
        "cjs" | "js" | "mjs" => '\u{e74e}',
        "class" | "jad" | "jar" | "java" | "war" => '\u{e256}',
        "clj" => '\u{e768}',
        "cljs" => '\u{e76a}',
        "cmake" | "mk" => '\u{e673}',
        "coffee" => '\u{f0f4}',
        "cr" => '\u{e62f}',
        "crdownload" | "download" | "fdmdownload" | "part" => '\u{f01da}',
        "cs" | "csproj" | "csx" => '\u{f031b}',
        "cshtml" | "razor" => '\u{f1fa}',
        "css" => '\u{e749}',
        "csv" | "fods" | "gsheet" | "ods" | "tsv" | "xlr" | "xls" | "xlsm" | "xlsx" => '\u{f1c3}',
        "cts" | "mts" | "ts" => '\u{e628}',
        "cu" => '\u{e64b}',
        "cue" | "m3u" | "m3u8" => '\u{f0cb9}',
        "d" | "di" => '\u{e7af}',
        "dart" => '\u{e798}',
        "db" | "dump" | "ldb" | "mdb" | "prql" | "sql" => '\u{f1c0}',
        "deb" => '\u{e77d}',
        "desktop" => '\u{ebd1}',
        "diff" | "patch" => '\u{f440}',
        "djv" | "djvu" | "doc" | "docx" | "fodt" | "gdoc" | "odt" => '\u{f1c2}',
        "dll" | "lib" => '\u{eb9c}',
        "dmg" | "image" | "img" | "iso" | "qcow" | "qcow2" | "tc" | "vdi" | "vhd" | "vmdk" => {
            '\u{e271}'
        }
        "dot" | "gv" => '\u{f1049}',
        "dockerfile" | "dockerignore" => '\u{e650}',
        "drawio" => '\u{ebba}',
        "ebook" | "epub" | "mobi" => '\u{e28b}',
        "ebuild" => '\u{f30d}',
        "ejs" => '\u{e618}',
        "el" | "elc" => '\u{e632}',
        "elm" => '\u{e62c}',
        "eml" => '\u{f003}',
        "env" => '\u{f462}',
        "eps" | "fodg" | "odg" | "ps" | "svg" => '\u{f0559}',
        "erb" | "rubydoc" | "slim" => '\u{e73b}',
        "erl" => '\u{e7b1}',
        "ex" | "exs" => '\u{e62d}',
        "f" | "f90" | "for" => '\u{f121a}',
        "fnl" => '\u{e6af}',
        "fodp" | "gslides" | "odp" | "pps" | "ppsx" | "ppt" | "pptx" => '\u{f1c4}',
        "fs" | "fsi" | "fsproj" | "fsx" => '\u{e7a7}',
        "gcode" => '\u{f0af4}',
        "gd" | "godot" | "tres" | "tscn" => '\u{e65f}',
        "gem" | "gemfile" | "gemspec" | "guardfile" | "rb" | "rspec" | "rspec_parallel"
        | "rspec_status" | "ru" => '\u{e21e}',
        "gform" => '\u{f298}',
        "git" => '\u{f1d3}',
        "go" => '\u{e65e}',
        "gql" | "graphql" => '\u{e662}',
        "gradle" => '\u{e660}',
        "groovy" | "gvy" => '\u{e775}',
        "haml" => '\u{e664}',
        "hbs" | "mustache" => '\u{e60f}',
        "hex" => '\u{f12a7}',
        "hs" | "lhs" => '\u{e777}',
        "htm" | "html" | "shtml" | "xhtml" => '\u{f13b}',
        "ical" | "icalendar" | "ics" | "ifb" => '\u{eab0}',
        "iml" => '\u{e7b5}',
        "ipynb" => '\u{e678}',
        "jl" => '\u{e624}',
        "jmd" | "markdown" | "md" | "mkd" | "rdoc" | "rmd" => '\u{f48a}',
        "jsx" | "tsx" => '\u{e7ba}',
        "kbx" => '\u{f0bc4}',
        "kdb" | "kdbx" => '\u{f23e}',
        "key" | "p12" | "pem" | "pfx" => '\u{eb11}',
        "kt" | "kts" => '\u{e634}',
        "less" => '\u{e758}',
        "license" => '\u{f02d}',
        "lisp" => '\u{f0172}',
        "lock" => '\u{f023}',
        "log" => '\u{f18d}',
        "lua" => '\u{e620}',
        "magnet" => '\u{f076}',
        "md5" | "sha1" | "sha224" | "sha256" | "sha384" | "sha512" => '\u{f0565}',
        "mid" => '\u{f08f2}',
        "ml" | "mli" | "mll" | "mly" => '\u{e67a}',
        "nim" | "nimble" | "nims" => '\u{e677}',
        "ninja" => '\u{f0774}',
        "nix" => '\u{f313}',
        "node" => '\u{e718}',
        "odf" => '\u{f0784}',
        "org" => '\u{e633}',
        "out" => '\u{eb2c}',
        "pdf" => '\u{f1c1}',
        "phar" | "php" => '\u{e73d}',
        "pkg" => '\u{eb29}',
        "pl" | "plx" | "pm" | "pod" | "t" => '\u{e67e}',
        "pp" => '\u{e631}',
        "ps1" | "psd1" | "psm1" => '\u{ebc7}',
        "psd" => '\u{e7b8}',
        "pub" => '\u{f0dd6}',
        "purs" => '\u{e630}',
        "py" | "pyc" | "pyd" | "pyi" | "pyo" | "whl" => '\u{e606}',
        "r" | "rdata" | "rds" => '\u{e68a}',
        "rdb" => '\u{e76d}',
        "readme" => '\u{f00ba}',
        "rkt" | "scm" | "sld" | "ss" => '\u{e6b1}',
        "rlib" | "rmeta" | "rs" => '\u{e68b}',
        "rpm" => '\u{e7bb}',
        "rss" => '\u{f09e}',
        "rst" | "rtf" | "txt" => '\u{f15c}',
        "sal" | "sr" => '\u{f147b}',
        "sass" | "scss" => '\u{e603}',
        "sbt" | "srt" | "ssa" | "sub" => '\u{f0a16}',
        "scala" => '\u{e737}',
        "service" => '\u{eba2}',
        "sig" | "signature" => '\u{f19c3}',
        "sln" => '\u{e70c}',
        "sqlite3" => '\u{e7c4}',
        "styl" | "stylus" => '\u{e600}',
        "sublime-build" | "sublime-keymap" | "sublime-menu" | "sublime-options"
        | "sublime-package" | "sublime-project" | "sublime-session" | "sublime-settings"
        | "sublime-snippet" | "sublime-theme" => '\u{e7aa}',
        "svelte" => '\u{e697}',
        "swift" => '\u{e755}',
        "tf" | "tfstate" | "tfvars" => '\u{f1062}',
        "torrent" => '\u{e275}',
        "twig" => '\u{e61c}',
        "typ" => '\u{1d42d}',
        "unity" | "unity3d" => '\u{e721}',
        "v" => '\u{e6ac}',
        "vim" => '\u{e7c5}',
        "vue" => '\u{f0844}',
        "xml" | "xul" => '\u{f05c0}',
        "yaml" | "yml" => '\u{e6a8}',
        "zig" => '\u{e6a9}',
        "zsh-theme" => '\u{f1183}',
        _ => return None,
    })
}

pub(super) fn name_style(name: &str) -> Option<&'static str> {
    Some(match name {
        "Brewfile" | "bsconfig.json" | "BUILD" | "BUILD.bazel" | "build.gradle" | "build.sbt"
        | "build.xml" | "Cargo.toml" | "CMakeLists.txt" | "composer.json" | "configure"
        | "Containerfile" | "Dockerfile" | "Earthfile" | "flake.nix" | "Gemfile"
        | "GNUmakefile" | "Gruntfile.coffee" | "Gruntfile.js" | "jsconfig.json" | "Justfile"
        | "justfile" | "Makefile" | "makefile" | "meson.build" | "mix.exs" | "package.json"
        | "Pipfile" | "PKGBUILD" | "Podfile" | "pom.xml" | "Procfile" | "pyproject.toml"
        | "Rakefile" | "RoboFile.php" | "SConstruct" | "tsconfig.json" | "Vagrantfile"
        | "webpack.config.cjs" | "webpack.config.js" | "WORKSPACE" => "1;4;33",
        "id_dsa" | "id_ecdsa" | "id_ecdsa_sk" | "id_ed25519" | "id_ed25519_sk" | "id_rsa" => "1;32",
        _ => return None,
    })
}

pub(super) fn extension_style(name: &str) -> Option<&'static str> {
    Some(match name {
        "ninja" => "1;4;33",
        "arw" | "avif" | "bmp" | "cbr" | "cbz" | "cr2" | "dvi" | "eps" | "fodg" | "gif"
        | "heic" | "heif" | "ico" | "j2c" | "j2k" | "jfi" | "jfif" | "jif" | "jp2" | "jpe"
        | "jpeg" | "jpf" | "jpg" | "jpx" | "jxl" | "nef" | "odg" | "orf" | "pbm" | "pgm"
        | "png" | "pnm" | "ppm" | "ps" | "psd" | "pxm" | "raw" | "qoi" | "svg" | "tif" | "tiff"
        | "webp" | "xcf" | "xpm" => "35",
        "avi" | "flv" | "h264" | "heics" | "m2ts" | "m2v" | "m4v" | "mkv" | "mov" | "mp4"
        | "mpeg" | "mpg" | "ogm" | "ogv" | "video" | "vob" | "webm" | "wmv" => "1;35",
        "aac" | "m4a" | "mka" | "mp2" | "mp3" | "ogg" | "opus" | "wma" => "36",
        "aif" | "aifc" | "aiff" | "alac" | "ape" | "flac" | "pcm" | "wav" | "wv" => "1;36",
        "age" | "asc" | "cer" | "crt" | "csr" | "gpg" | "kbx" | "md5" | "p12" | "pem" | "pfx"
        | "pgp" | "pub" | "sha1" | "sha224" | "sha256" | "sha384" | "sha512" | "sig"
        | "signature" => "1;32",
        "djvu" | "doc" | "docx" | "eml" | "fodp" | "fods" | "fodt" | "fotd" | "gdoc" | "key"
        | "keynote" | "numbers" | "odp" | "ods" | "odt" | "pages" | "pdf" | "ppt" | "pptx"
        | "rtf" | "xls" | "xlsm" | "xlsx" => "32",
        "7z" | "ar" | "arj" | "br" | "bz" | "bz2" | "bz3" | "cpio" | "deb" | "dmg" | "gz"
        | "iso" | "lz" | "lz4" | "lzh" | "lzma" | "lzo" | "phar" | "qcow" | "qcow2" | "rar"
        | "rpm" | "tar" | "taz" | "tbz" | "tbz2" | "tc" | "tgz" | "tlz" | "txz" | "tz" | "xz"
        | "vdi" | "vhd" | "vhdx" | "vmdk" | "z" | "zip" | "zst" => "31",
        "bak" | "bk" | "bkp" | "crdownload" | "download" | "fdmdownload" | "part" | "swn"
        | "swo" | "swp" | "tmp" => "2",
        "a" | "bundle" | "class" | "cma" | "cmi" | "cmo" | "cmx" | "dll" | "dylib" | "elc"
        | "elf" | "ko" | "lib" | "o" | "obj" | "pyc" | "pyd" | "pyo" | "so" | "zwc" => "33",
        "applescript" | "as" | "asa" | "awk" | "c" | "c++" | "cabal" | "cc" | "clj" | "cp"
        | "cpp" | "cr" | "cs" | "css" | "csx" | "cu" | "cxx" | "cypher" | "d" | "dart" | "di"
        | "dpr" | "el" | "elm" | "erl" | "ex" | "exs" | "f" | "f90" | "fnl" | "for" | "fs"
        | "fsh" | "fsi" | "fsx" | "gd" | "go" | "gradle" | "groovy" | "gvy" | "h" | "h++"
        | "hh" | "hpp" | "hs" | "htc" | "hxx" | "inc" | "inl" | "ipynb" | "java" | "jl" | "js"
        | "jsx" | "kt" | "kts" | "kusto" | "less" | "lhs" | "lisp" | "ltx" | "lua" | "m"
        | "malloy" | "matlab" | "ml" | "mli" | "mn" | "nb" | "p" | "pas" | "php" | "pl" | "pm"
        | "pod" | "pp" | "prql" | "ps1" | "psd1" | "psm1" | "purs" | "py" | "r" | "rb" | "rs"
        | "rq" | "sass" | "scala" | "scm" | "scss" | "sld" | "sql" | "ss" | "swift" | "tcl"
        | "tex" | "ts" | "v" | "vb" | "vsh" | "zig" => "1;33",
        _ => return None,
    })
}
