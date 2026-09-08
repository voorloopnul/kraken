//! The clipboard's image.
//!
//! This is the one place Kraken reaches into Qt's C++ directly. QML has no
//! clipboard API at all, and the workaround the rest of the app uses — a hidden
//! `TextEdit` that copies and pastes on our behalf, in `qml/common/Clipboard.qml`
//! — can only carry text: `TextEdit::paste` asks the clipboard for a string, so
//! a screenshot on it arrives as nothing. An image has to come from
//! `QClipboard` itself, and the Rust bindings expose no way to it.
//!
//! Everything else about an attachment stays on the Rust side of the seam, so
//! what crosses here is as small as it can be: bytes out, nothing in.

use cpp::cpp;
use qmetaobject::QByteArray;

cpp! {{
    #include <QtCore/QBuffer>
    #include <QtCore/QByteArray>
    #include <QtGui/QClipboard>
    #include <QtGui/QGuiApplication>
    #include <QtGui/QImage>
}}

/// The clipboard's image, encoded as PNG, or `None` when it holds none.
///
/// PNG rather than the source encoding because the clipboard does not keep one:
/// what is on it is pixels — a screenshot tool, a browser's "copy image" and
/// another Qt application all put a `QImage` there, not a file — so something
/// has to choose, and PNG is lossless and accepted everywhere we send images.
pub fn image_png() -> Option<Vec<u8>> {
    let encoded = cpp!(unsafe [] -> QByteArray as "QByteArray" {
        const QImage image = QGuiApplication::clipboard()->image();
        if (image.isNull())
            return QByteArray();
        QByteArray png;
        QBuffer buffer(&png);
        buffer.open(QIODevice::WriteOnly);
        if (!image.save(&buffer, "PNG"))
            return QByteArray();
        return png;
    });
    let bytes = encoded.to_slice();
    (!bytes.is_empty()).then(|| bytes.to_vec())
}
