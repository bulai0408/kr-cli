import XCTest

final class LaunchUITests: XCTestCase {
    func testShowsGreeting() {
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.staticTexts["Root.text.greeting"].waitForExistence(timeout: 10))
    }

    /// Fails while the file named by TEST_RUNNER_KR_FAIL_ONCE (a host path) does not
    /// exist, creating it: fails on the first run, passes on the rerun.
    func testFlakyOnce() throws {
        guard let marker = ProcessInfo.processInfo.environment["KR_FAIL_ONCE"] else { return }
        if !FileManager.default.fileExists(atPath: marker) {
            FileManager.default.createFile(atPath: marker, contents: Data())
            XCTFail("first run fails on purpose")
        }
    }
}

final class SettingsUITests: XCTestCase {
    func testLaunchesTwice() {
        let app = XCUIApplication()
        app.launch()
        app.terminate()
        app.launch()
        XCTAssertTrue(app.staticTexts["Root.text.greeting"].waitForExistence(timeout: 10))
    }
}
