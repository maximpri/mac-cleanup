# SPDX-License-Identifier: GPL-3.0-or-later
# Homebrew formula for the maximpri/homebrew-diskray tap. The release workflow
# opens a pull request in the tap that updates `url` and `sha256`.
class Diskray < Formula
  desc "X-ray for your Mac's disk: understand before you delete"
  homepage "https://github.com/maximpri/diskray"
  url "https://github.com/maximpri/diskray/releases/download/v0.4.0/diskray-0.4.0.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "GPL-3.0-or-later"
  head "https://github.com/maximpri/diskray.git", branch: "main"

  depends_on "rust" => :build
  depends_on :macos

  def install
    system "cargo", "install", *std_cargo_args

    # Local AI needs Apple silicon, macOS 26 (Tahoe) or later, and an SDK
    # with FoundationModels. Everything else works without the helper.
    sdk = Utils.safe_popen_read("xcrun", "--show-sdk-path").chomp
    if Hardware::CPU.arm? && MacOS.version >= :tahoe &&
       File.directory?("#{sdk}/System/Library/Frameworks/FoundationModels.framework")
      libexec.mkpath
      system "xcrun", "swiftc", "-O", "-parse-as-library",
             "-target", "arm64-apple-macosx26.0",
             "native/AIHelper.swift", "-o", libexec/"diskray-ai"
    end
  end

  def caveats
    <<~EOS
      Start with:
        diskray why       # one-screen explanation of what fills the disk
        diskray           # interactive review

      Local AI features need Apple Intelligence turned on in System Settings,
      with its model downloaded, on Apple silicon and macOS 26 or later.

      Full Disk Access is optional. Grant it to your terminal only if you want
      Diskray to measure protected folders:
        System Settings > Privacy & Security > Full Disk Access

      Connect coding agents (read-only tools):
        claude mcp add --scope user diskray -- diskray mcp
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/diskray --version")
    (testpath/"pack.toml").write <<~TOML
      schema = 1
      pack = "test"

      [[rule]]
      id = "cache"
      label = "Test cache"
      path = "Library/Caches/com.example.diskray-test"
      tier = "routine"
      note = "rebuilt automatically"
    TOML
    assert_match "1 valid rule", shell_output("#{bin}/diskray rules check #{testpath}/pack.toml")
    request = '{"jsonrpc":"2.0","id":1,"method":"server/discover",' \
              '"params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}'
    output = pipe_output("#{bin}/diskray mcp", "#{request}\n", 0)
    assert_match "\"resultType\":\"complete\"", output
  end
end
