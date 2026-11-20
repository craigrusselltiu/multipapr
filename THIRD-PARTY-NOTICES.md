# Third-party notices

Multipapr's own code is under the [MIT License](LICENSE). The release `multipapr.exe` also contains the following.

## mpv

`multipapr.exe` bundles an unmodified `mpv.exe` and `d3dcompiler_43.dll` from a portable mpv build. On first use it unpacks them and runs `mpv.exe` as a separate program.

- **License:** GNU General Public License. This build includes GPL components. See [licenses/GPL-3.0.txt](licenses/GPL-3.0.txt).
- **Version:** mpv v0.42.0-38-gbcde6b93c
- **Source:** [mpv at commit bcde6b93c](https://github.com/mpv-player/mpv/tree/bcde6b93c). The build scripts and dependency sources are in [shinchiro/mpv-winbuild-cmake, release 20261111](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20261111).
- `d3dcompiler_43.dll` is Microsoft's redistributable shader compiler, shipped unmodified as part of that build.

## Inter

The editor UI uses the Inter typeface, © The Inter Project Authors, under the SIL Open Font License 1.1. See [assets/fonts/LICENSE.txt](assets/fonts/LICENSE.txt).
