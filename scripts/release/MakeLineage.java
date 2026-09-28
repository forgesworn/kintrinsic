// MakeLineage: build an APK Signature Scheme v3 proof-of-rotation lineage
// (old -> new) holding only the OLD private key and the NEW public certificate.
//
// Why this works: in apksig, SigningCertificateLineage.spawnDescendant(parent,
// child, caps) signs the new node's signed data (child cert + signature
// algorithm id) with the PARENT's KeyConfig only; from the child it reads just
// getCertificate(). The new private key is never touched, so we hand it null.
//
// Capabilities are fixed to the Kintrinsic rotation plan (S2): the old signer
// keeps installed-data only; shared-uid, permission, rollback and auth are off.
// The new signer gets apksigner's defaults (all true), as `apksigner rotate` does.
//
// Build and run (Java 11+, single-file launch):
//   AS_JAR=~/Android/Sdk/build-tools/36.0.0/lib/apksigner.jar
//   java -cp "$AS_JAR" MakeLineage.java <old-keystore> <old-alias> <out.lineage> <new-cert.pem|.der>
// The old keystore password is read from the env var OLD_KS_PASS (the key
// password from OLD_KEY_PASS, defaulting to the store password), so it never
// appears on the command line. JKS and PKCS12 are both detected automatically.

import com.android.apksig.SigningCertificateLineage;
import com.android.apksig.SigningCertificateLineage.SignerCapabilities;
import com.android.apksig.SigningCertificateLineage.SignerConfig;

import java.io.File;
import java.io.FileInputStream;
import java.io.InputStream;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;

public class MakeLineage {
    private static final int MIN_SDK = 28; // v3 rotation, Android 9+

    public static void main(String[] args) throws Exception {
        if (args.length != 4) {
            System.err.println("usage: MakeLineage <old-keystore> <old-alias> <out.lineage> <new-cert.pem|.der>");
            System.exit(2);
        }
        String ksPass = System.getenv("OLD_KS_PASS");
        if (ksPass == null) {
            System.err.println("OLD_KS_PASS is not set");
            System.exit(2);
        }
        String keyPass = System.getenv().getOrDefault("OLD_KEY_PASS", ksPass);

        KeyStore ks = KeyStore.getInstance(new File(args[0]), ksPass.toCharArray());
        PrivateKey oldKey = (PrivateKey) ks.getKey(args[1], keyPass.toCharArray());
        X509Certificate oldCert = (X509Certificate) ks.getCertificate(args[1]);
        if (oldKey == null || oldCert == null) {
            throw new IllegalArgumentException("alias '" + args[1] + "' has no private key entry");
        }

        X509Certificate newCert;
        try (InputStream in = new FileInputStream(args[3])) {
            newCert = (X509Certificate) CertificateFactory.getInstance("X.509").generateCertificate(in);
        }
        if (newCert.equals(oldCert)) {
            throw new IllegalArgumentException("new certificate is the same as the old one");
        }

        SignerConfig oldSigner = new SignerConfig.Builder(oldKey, oldCert).build();
        // Certificate only: the child's private key is never used to build a lineage.
        SignerConfig newSigner = new SignerConfig.Builder((PrivateKey) null, newCert).build();

        SignerCapabilities oldCaps = new SignerCapabilities.Builder()
                .setInstalledData(true)
                .setSharedUid(false)
                .setPermission(false)
                .setRollback(false)
                .setAuth(false)
                .build();

        SigningCertificateLineage lineage = new SigningCertificateLineage.Builder(oldSigner, newSigner)
                .setMinSdkVersion(MIN_SDK)
                .setOriginalCapabilities(oldCaps)
                .build();

        lineage.writeToFile(new File(args[2]));
        System.out.println("wrote " + args[2] + " with " + lineage.size() + " signers");
    }
}
