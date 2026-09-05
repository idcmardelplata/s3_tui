# Maintainer: Martin Algañaraz <idcmardelplata@protonmail.com>

pkgname=s3_tui
pkgver=0.5.2
pkgrel=1
pkgdesc="A terminal UI manager for AWS S3"
arch=('x86_64' 'aarch64')
url="https://github.com/idcmardelplata/s3_tui"
license=('MIT')
depends=('glibc' 'gcc-libs')
makedepends=('cargo' 'rust' 'git')
provides=('s3-tui')
conflicts=('s3-tui')
source=("${pkgname}::git+${url}.git#tag=v${pkgver}")
sha256sums=('SKIP')
options=('!lto')

pkgver() {
    printf '%s' "${pkgver}"
}

prepare() {
  cd "${srcdir}/${pkgname}"
  cargo fetch --locked
}

build() {
  cd "${srcdir}/${pkgname}"
  cargo build --release --locked
}

check() {
  cd "${srcdir}/${pkgname}"
  cargo test --release --locked --frozen
}

package() {
  cd "${srcdir}/${pkgname}"

  # Main binary
  install -Dm755 "target/release/${pkgname}" \
    "${pkgdir}/usr/bin/${pkgname}"

  # README, license and example config
  install -Dm644 README.md "${pkgdir}/usr/share/doc/${pkgname}/README.md"
  install -Dm644 LICENSE "${pkgdir}/usr/share/licenses/${pkgname}/LICENSE"
  install -Dm644 config.toml.example \
    "${pkgdir}/usr/share/doc/${pkgname}/config.toml.example"
}

# vim:set ts=2 sw=2 et:
