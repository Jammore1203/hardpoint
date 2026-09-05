# Maintainer: see README

pkgname=hardpoint
pkgver=1.0.0
pkgrel=1
pkgdesc="Operation Ironveil: a multiplayer arena FPS in the style of a 2002 console shooter"
arch=('x86_64' 'aarch64')
url="https://github.com/Jammore1203/hardpoint"
license=('custom')
depends=('vulkan-icd-loader' 'alsa-lib')
makedepends=('rust' 'cargo')
optdepends=('vulkan-radeon: AMD graphics'
            'nvidia-utils: NVIDIA graphics'
            'vulkan-intel: Intel graphics')
backup=('etc/hardpoint/server.conf')
source=()

build() {
  cd "$startdir"
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR="$startdir/target"
  make build
}

check() {
  cd "$startdir"
  ./target/release/hardpoint --audit
  ./target/release/hardpoint --stairs ALL
}

package() {
  cd "$startdir"
  make DESTDIR="$pkgdir" PREFIX=/usr UNITDIR="$pkgdir/usr/lib/systemd/system" install
}
