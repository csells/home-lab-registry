
import XCTest
final class SmokeUITests: XCTestCase {
  func testTapUpdatesRenderedState() throws {
    let app = XCUIApplication()
    app.launch()
    let result = app.staticTexts["result"]
    XCTAssertTrue(result.waitForExistence(timeout: 15))
    XCTAssertEqual(result.label, "Ready")
    app.buttons["verify-button"].tap()
    XCTAssertEqual(result.label, "Tap verified")
    let evidence = XCTAttachment(screenshot: app.screenshot())
    evidence.name = "Verified tap"
    evidence.lifetime = .keepAlways
    add(evidence)
  }
}
