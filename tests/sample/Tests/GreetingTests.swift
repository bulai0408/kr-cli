import Testing
@testable import KrSample

struct GreetingTests {
    @Test func greetsByName() {
        #expect(Greeting.text(for: "kr") == "Hello, kr")
    }

    @Test func greetsEmptyName() {
        #expect(Greeting.text(for: "") == "Hello, ")
    }
}
