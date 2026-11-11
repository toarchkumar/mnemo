import Foundation
import Mnemo
import NaturalLanguage

/// Choosing between Apple's embedders when a store is created.
///
/// A store's vector width is fixed when it's created, and the file records
/// its embedder's identity. Opening it with a different model (another
/// kind, or another revision of the same one) fails with
/// `MnemoError.Invalid` ("embedder identity mismatch"). So there's no
/// fallback from one model to another on a live store: pick one when you
/// create the store and keep it.
///
/// To move an app to a better model later (say the contextual model's
/// assets have arrived), create a new store with the new embedder and
/// re-add its memories from your app's own data with `rememberText`. Name
/// each store after its embedder, so the two never collide; the iOS
/// sample's notes do this. The old store keeps working with its own
/// embedder until you delete it.
public enum AppleEmbedders {
    /// The best Apple embedder this device can run now, without a
    /// download: the contextual model when its assets are on the device
    /// (iOS 17 or macOS 14 and later), else the sentence model, else `nil`.
    public static func bestAvailable(language: NLLanguage = .english) -> (any MnemoEmbedder)? {
        if #available(iOS 17.0, macOS 14.0, *),
           let contextual = AppleContextualEmbedder(language: language),
           contextual.hasAvailableAssets {
            return contextual
        }
        return AppleSentenceEmbedder(language: language)
    }
}
