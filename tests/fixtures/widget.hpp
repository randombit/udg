// Self-contained fixture (no includes) exercising extraction: doc
// comments, attribute macros, access filtering, enums, templates.

// Expand to the same attribute shapes Botan's real macros produce, so
// declaration extents include the spelled macro tokens.
#define TEST_API(maj, min) __attribute__((visibility("default")))
#define TEST_DEPRECATED(msg) [[deprecated(msg)]]
#define WIDGET_GUARD_H_

/// Maximum widgets per frob.
#define WIDGET_LIMIT 64

#define WIDGET_DEFAULT_PAD 3 ///< Pad bytes when unspecified.

/// Compute padding for size n.
/// @param n the unpadded size
#define WIDGET_PAD(n) ((n) + WIDGET_DEFAULT_PAD)

namespace demo {

/**
* A widget that frobs.
*
* Extended discussion of widgets
* across two lines.
*
* @see Gadget
*/
class TEST_API(2, 3) Widget {
   public:
      /**
      * Frob the widget.
      * @param amount how much to frob
      * @param fast whether to hurry
      * @return the frob count
      */
      virtual unsigned long frob(unsigned long amount, bool fast = true) const = 0;

      /// Old way to frob.
      TEST_DEPRECATED("Use frob") unsigned long old_frob() { return 0; }

      /// Even older way to frob.
      TEST_DEPRECATED("Use frob_{fast,slow}") unsigned long old_frob2();

      /// Fixed frob geometry.
      enum {
         BLOCK_SIZE = 16,
         KEY_SIZE = 32, /**< Bytes of key material. */
      };

      virtual ~Widget() = default;

      Widget(const Widget& other) = delete;
      Widget& operator=(const Widget& other) = delete;

   protected:
      /// Helper for subclasses.
      void helper() noexcept;

   private:
      int hidden_value;
      void hidden_fn();
};

/// A concrete widget.
class SteelWidget : public Widget {
   public:
      unsigned long frob(unsigned long amount, bool fast = true) const override;

      /// Frob, but in bulk.
      unsigned long frob_many(unsigned long n) const;
};

/**
* Operating mode.
*/
enum class Mode : unsigned char {
   On = 0,
   Off = 1,
   Legacy TEST_DEPRECATED("Use Off") = Off,
};

/// Make a widget by name.
Widget* make_widget(const char* name);

/// A generic holder.
template <typename T, unsigned long N>
class Holder {
   public:
      /// Capacity in elements.
      enum { CAP = N };

      /// Build from parts.
      template <typename... Args>
      explicit Holder(Args&&... parts);

      ~Holder();

      /// Get element i.
      T get(unsigned long i) const;
};

/// Alias for a widget pointer.
using WidgetPtr = Widget*;

/// Scratch space needed by frobbers.
enum : unsigned char { WORKSPACE_SIZE = 8 };

/// Severity for logging.
typedef enum { LEVEL_LOW = 1, LEVEL_HIGH = 2 } level_t;

}  // namespace demo
