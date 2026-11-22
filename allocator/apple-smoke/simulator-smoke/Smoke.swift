
import UIKit
@main class App: UIResponder, UIApplicationDelegate {
  func application(_ application: UIApplication, configurationForConnecting session: UISceneSession,
                   options: UIScene.ConnectionOptions) -> UISceneConfiguration {
    let config = UISceneConfiguration(name: nil, sessionRole: session.role)
    config.delegateClass = Scene.self
    return config
  }
}
final class Scene: UIResponder, UIWindowSceneDelegate {
  var window: UIWindow?
  private let label = UILabel()
  func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
    guard let screen = scene as? UIWindowScene else { return }
    let controller = UIViewController()
    controller.view.backgroundColor = .white
    label.text = "Ready"
    label.textColor = .black
    label.accessibilityIdentifier = "result"
    let button = UIButton(type: .system)
    button.setTitle("Verify tap", for: .normal)
    button.accessibilityIdentifier = "verify-button"
    button.addTarget(self, action: #selector(verify), for: .touchUpInside)
    let stack = UIStackView(arrangedSubviews: [label, button])
    stack.axis = .vertical
    stack.alignment = .center
    stack.spacing = 24
    stack.translatesAutoresizingMaskIntoConstraints = false
    controller.view.addSubview(stack)
    NSLayoutConstraint.activate([stack.centerXAnchor.constraint(equalTo: controller.view.centerXAnchor),
                                 stack.centerYAnchor.constraint(equalTo: controller.view.centerYAnchor)])
    let window = UIWindow(windowScene: screen)
    window.rootViewController = controller
    window.makeKeyAndVisible()
    self.window = window
  }
  @objc private func verify() { label.text = "Tap verified" }
}
