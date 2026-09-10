class Tmail < Formula
  desc "Gmail-inspired, keyboard-first terminal email client backed by the Himalaya CLI"
  homepage "https://github.com/jodaka/tmail"
  version "0.2.1"

  livecheck do
    url :stable
    regex(/^tmail[._-]v?(\d+(?:\.\d+)+)[._-]/i)
    strategy :github_latest
  end

  depends_on "himalaya"

  on_macos do
    on_arm do
      url "https://github.com/jodaka/tmail/releases/download/v#{version}/tmail-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "96a5b05b429d23c649d631daf353a1b720713b1e52cc3f2591cce65005073515"
    end
    on_intel do
      url "https://github.com/jodaka/tmail/releases/download/v#{version}/tmail-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "f93a061701683201dc58151c43d043213142f08eb8c0dc6d4faf2bd6ab987686"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/jodaka/tmail/releases/download/v#{version}/tmail-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "12c2c3a5ec6273b1096c2b0433e8ac73ee7b6e91a32d7fc426e282574299777e"
    end
  end

  def install
    bin.install "tmail"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tmail --version")
  end
end
