---
name: msbuild
description: Build Visual Studio C/C++ projects from CLI using MSBuild. Use this skill whenever the user asks to build, compile, or rebuild a Visual Studio solution or project — including "빌드해줘", "build this", "compile", "rebuild", or when you need to verify a build after code changes. Also triggers when build errors need diagnosis. This skill handles MSBuild path detection, bash switch syntax, NuGet restore, and Configuration/Platform selection automatically.
---

# MSBuild CLI Build Skill

Build Visual Studio solutions and projects from bash on Windows without requiring the VS IDE.

## Why MSBuild over devenv.com

- `msbuild.exe` runs independently of the VS IDE — no conflict even if VS is open with the same solution
- Faster than `devenv.com` (no IDE process overhead)
- Standard CLI switches (`/p:`, `/t:`, `/v:`) with rich control
- The right tool for CI/CD and automated builds

## Step 1: Find MSBuild

MSBuild lives inside the VS installation. On this machine the path is:

```
/c/Program Files/Microsoft Visual Studio/2022/Professional/MSBuild/Current/Bin/MSBuild.exe
```

If you're unsure of the edition (Professional, Enterprise, Community), detect it:

```bash
ls "/c/Program Files/Microsoft Visual Studio/2022/"
```

Store the path in a variable for reuse:

```bash
MSBUILD="/c/Program Files/Microsoft Visual Studio/2022/Professional/MSBuild/Current/Bin/MSBuild.exe"
```

## Step 2: Find the build target

Look for `.sln` files in the project root:

```bash
ls *.sln
```

**Prefer building the `.sln` over individual `.vcxproj`** — many projects use `$(SolutionDir)` in include/library paths, which only resolves correctly when building through the solution.

## Step 3: Determine Configuration and Platform

Parse available configurations from the `.sln` file:

```bash
grep "Debug\|Release" *.sln | head -20
```

Look for the `GlobalSection(SolutionConfigurationPlatforms)` block. Common combinations:

| Configuration | Platform values |
|---|---|
| Debug, Release | x64, x86, Win32, ARM64 |

**Platform naming:** Some projects use `Win32` instead of `x86`. The `.sln` file is authoritative — use exactly what it says.

If the user doesn't specify, ask which Configuration|Platform they want. If context makes it obvious (e.g., "릴리즈로 빌드해줘, 32bit"), map accordingly.

## Step 4: Build

### Bash switch syntax (critical)

In bash on Windows, MSBuild's `/` switches get interpreted as paths. Use `//` prefix instead:

```bash
# WRONG — bash interprets /p: as a path
"$MSBUILD" solution.sln /p:Configuration=Release /p:Platform=x86

# CORRECT — double slash escapes for bash
"$MSBUILD" solution.sln //p:Configuration=Release //p:Platform=x86 //v:minimal
```

### Build commands

**Incremental build (default):**

```bash
"$MSBUILD" "path/to/solution.sln" //p:Configuration=Release //p:Platform=x86 //v:minimal 2>&1
```

**Rebuild (clean + build):**

```bash
"$MSBUILD" "path/to/solution.sln" //t:Rebuild //p:Configuration=Release //p:Platform=x86 //v:minimal 2>&1
```

**Clean:**

```bash
"$MSBUILD" "path/to/solution.sln" //t:Clean //p:Configuration=Release //p:Platform=x86 //v:minimal 2>&1
```

**Single project within a solution** (when only one project needs building):

```bash
"$MSBUILD" "path/to/solution.sln" //t:ProjectName //p:Configuration=Release //p:Platform=x86 //v:minimal 2>&1
```

Note: project names with dots or hyphens need those replaced with underscores in the `/t:` target. For example, `alad2-dll` becomes `alad2_dll`.

### Verbosity levels

| Flag | Level | Use when |
|---|---|---|
| `//v:quiet` | Errors only | Quick pass/fail check |
| `//v:minimal` | Errors + summary | **Default — use this** |
| `//v:normal` | Standard output | Diagnosing build issues |
| `//v:detailed` | Full detail | Deep debugging |

## Step 5: Handle common issues

### NuGet packages not found

If you see `fatal error C1083: Cannot open include file`, packages may need restoring:

```bash
# Check if packages directory exists
ls packages/ 2>/dev/null

# Restore via MSBuild
"$MSBUILD" "path/to/solution.sln" //t:Restore //v:minimal 2>&1

# If MSBuild restore doesn't work (native C++ projects with packages.config),
# try nuget CLI if available:
nuget restore "path/to/solution.sln"
```

### vcpkg packages not found

If you see `Cannot open include file` for libraries like spdlog, fmt, nlohmann-json etc., the project likely uses vcpkg. Check for a `vcpkg.json` manifest in the project root.

```bash
# Check if project uses vcpkg
ls vcpkg.json 2>/dev/null

# Enable vcpkg MSBuild integration (one-time setup per machine)
# VS2022 bundles vcpkg — use that:
"/c/Program Files/Microsoft Visual Studio/2022/Professional/VC/vcpkg/vcpkg.exe" integrate install

# Then rebuild — vcpkg manifest mode will auto-download packages
"$MSBUILD" "path/to/solution.sln" //p:Configuration=Release //p:Platform=x64 //v:minimal 2>&1
```

If vcpkg is not bundled with VS, check for a standalone installation (`where vcpkg` or look in common paths like `C:\vcpkg`).

### $(SolutionDir) errors

If building a `.vcxproj` directly fails with path errors but the solution builds fine — this is expected. Always build via the `.sln`.

### LNK errors (linker)

Common when building a single project that depends on outputs from other projects. Build the full solution instead, or build dependencies first.

### Platform mismatch

If you get "platform not found" errors, check the `.sln` for exact platform names. `x86` and `Win32` are NOT interchangeable in MSBuild — use exactly what the solution defines.

## Quick Reference

```bash
# One-liner: detect MSBuild + build Release x86
MSBUILD="$(ls "/c/Program Files/Microsoft Visual Studio/2022/"*/MSBuild/Current/Bin/MSBuild.exe 2>/dev/null | head -1)" && "$MSBUILD" "solution.sln" //p:Configuration=Release //p:Platform=x86 //v:minimal 2>&1
```
