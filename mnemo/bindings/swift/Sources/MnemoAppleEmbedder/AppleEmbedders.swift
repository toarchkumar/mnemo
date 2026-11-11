import Foundation
import Mnemo
import NaturalLanguage

/// Choosing between Apple's embedders when a store is created.
///
/// **A store is bound to the model that first wrote to it.** Its vector
/// width is fixed when it's created, and the file records the identity of
/// the first embedder attached to it (since v0.13, EMBED-ID-1). Opening it
/// with a different model, or another revision of the same one, fails with
/// `MnemoError.EmbedderMismatch` on the store's first call. So there's no
/// fallback from one model to another on a live store: pick one when you
/// create the store, and keep it.
///
/// To move an app to a better model later (say the contextual model's
/// assets have arrived, or the system ships a new revision), create a new
/// store with the new embedder and add its memories again from your app's
/// own data with `rememberText`. Name each store after its embedder's
/// identity, so the two never collide; the iOS sample's notes do this. The
/// old store keeps working with its own embedder until you delete it. (An
/// engine-side re-embed of an existing store is planned, not shipped.)
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
