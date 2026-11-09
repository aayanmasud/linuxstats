# Maintainer: Aayan Masud <aayanmasud@outlook.com>
pkgname=linuxstats
pkgver=0.1.2
pkgrel=1
pkgdesc="A desktop system monitor for Linux"
arch=('x86_64' 'aarch64')
url="https://github.com/aayanmasud/linuxstats"
license=('MIT')
depends=('glibc')
makedepends=('cargo' 'rust' 'git' 'make' 'gcc')
source=("${pkgname}-${pkgver}.tar.gz::https://github.com/aayanmasud/linuxstats/archive/refs/tags/v${pkgver}.tar.gz")
sha256sums=('SKIP')

prepare() {
  cd "$srcdir/${pkgname}-${pkgver}"
  cargo fetch --locked
}

build() {
  cd "$srcdir/${pkgname}-${pkgver}"
  cargo build --release --locked
}

package() {
  cd "$srcdir/${pkgname}-${pkgver}"
  install -Dm755 "target/release/linuxstats" "$pkgdir/usr/bin/linuxstats"
  install -Dm644 "assets/linuxstats.desktop" "$pkgdir/usr/share/applications/linuxstats.desktop"
}
